//! Consolidation: asking an engine to distil what it holds into beliefs.
//!
//! Engines that keep a cognitive layer (CortexDB's beliefs and facts) build
//! it from raw events — documents, conversation turns — with a model, which
//! takes seconds to minutes. That work never belongs on an agent's live turn,
//! so the contract only lets a host *ask* for it:
//! [`MemoryEngine::consolidate`](crate::MemoryEngine::consolidate) names a
//! [`ConsolidateRequest`] (which part of the tree, which kinds) and returns as
//! soon as the engine has taken the job, or once a quick build is done. What
//! it builds comes back through the engine's reads (on CortexDB, the derived
//! layers its answer route reads).
//!
//! An engine says how it consolidates in
//! [`EngineDescriptor::consolidation`](crate::EngineDescriptor::consolidation):
//!
//! - [`Consolidation::None`] — it does not; `consolidate` fails
//!   [`Error::Unsupported`].
//! - [`Consolidation::OnDemand`] — `consolidate` starts (or runs) a build and
//!   answers [`ConsolidateStatus::Started`] or
//!   [`ConsolidateStatus::Completed`].
//! - [`Consolidation::Scheduled`] — the engine consolidates on its own
//!   schedule; `consolidate` acknowledges with
//!   [`ConsolidateStatus::Scheduled`] and does nothing more.
//!
//! **Reading beliefs.** An engine that keeps what it builds apart from its
//! stored items (CortexDB's belief layer) serves it through
//! [`MemoryEngine::beliefs`](crate::MemoryEngine::beliefs): a
//! [`BeliefsRequest`] names a reach, an optional query and a limit, and the
//! answer is learning hits tagged [`BELIEF_TAG`]. They are not stored items,
//! so they cannot be listed, fetched or forgotten by id; forgetting the items
//! they were built from removes them. An engine whose beliefs are ordinary
//! learning items (the reference engine) has nothing to add and keeps the
//! default, which holds none.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::item::ItemKind;
use crate::namespace::Reach;

/// How an engine turns raw memory into beliefs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consolidation {
    /// It does not consolidate.
    #[default]
    None,
    /// [`crate::MemoryEngine::consolidate`] starts a build.
    OnDemand,
    /// The engine consolidates on its own schedule.
    Scheduled,
}

/// What to consolidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConsolidateRequest {
    /// The part of the namespace tree to consolidate. A subtree reach
    /// consolidates every node below `at`.
    pub reach: Reach,
    /// The item kinds whose items feed the build; empty means every kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<ItemKind>,
}

impl ConsolidateRequest {
    /// Consolidates everything `reach` admits.
    #[must_use]
    pub fn new(reach: Reach) -> Self {
        Self {
            reach,
            kinds: Vec::new(),
        }
    }

    /// Restricts the build to items of `kinds`.
    #[must_use]
    pub fn kinds(mut self, kinds: impl IntoIterator<Item = ItemKind>) -> Self {
        self.kinds = kinds.into_iter().collect();
        self
    }

    /// The kinds the build reads: [`ItemKind::ALL`] when none were named.
    #[must_use]
    pub fn admitted_kinds(&self) -> Vec<ItemKind> {
        if self.kinds.is_empty() {
            ItemKind::ALL.to_vec()
        } else {
            self.kinds.clone()
        }
    }

    /// Checks the request.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when a kind is named twice.
    pub fn validate(&self) -> Result<()> {
        let mut seen = Vec::with_capacity(self.kinds.len());
        for kind in &self.kinds {
            if seen.contains(kind) {
                return Err(Error::InvalidRequest(format!(
                    "consolidate names the kind `{}` twice",
                    kind.as_str()
                )));
            }
            seen.push(*kind);
        }
        Ok(())
    }
}

/// The tag on every hit [`MemoryEngine::beliefs`](crate::MemoryEngine::beliefs)
/// returns.
pub const BELIEF_TAG: &str = "belief";

/// Which beliefs to read (see [`MemoryEngine::beliefs`](crate::MemoryEngine::beliefs)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefsRequest {
    /// The part of the namespace tree whose beliefs to read.
    pub reach: Reach,
    /// What the beliefs should be relevant to. Without one, the most
    /// confident come first, then the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The most beliefs to return.
    pub limit: usize,
}

impl BeliefsRequest {
    /// At most `limit` beliefs within `reach`, most confident first.
    #[must_use]
    pub fn new(reach: Reach, limit: usize) -> Self {
        Self {
            reach,
            query: None,
            limit,
        }
    }

    /// Ranks the beliefs for `query`.
    #[must_use]
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Checks the request.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a zero limit or a blank query.
    pub fn validate(&self) -> Result<()> {
        if self.limit == 0 {
            return Err(Error::InvalidRequest(
                "a beliefs limit must be positive".to_string(),
            ));
        }
        if self.query.as_deref().is_some_and(|query| query.trim().is_empty()) {
            return Err(Error::InvalidRequest(
                "a beliefs query must not be blank".to_string(),
            ));
        }
        Ok(())
    }
}

/// How far a consolidation got before the call returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidateStatus {
    /// A build was started and runs in the background.
    Started,
    /// The engine consolidates on its own schedule; nothing was started.
    Scheduled,
    /// The build ran to completion within the call.
    Completed,
}

/// What [`crate::MemoryEngine::consolidate`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidateReceipt {
    /// How far it got.
    pub status: ConsolidateStatus,
    /// The engine's handles for the builds it started, when it names them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// How many engine-side scopes (nodes × kinds) the request covered.
    pub scopes: usize,
    /// How many beliefs the build produced, when it ran within the call and
    /// the engine reports the count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<usize>,
}

impl ConsolidateReceipt {
    /// A receipt for an engine that consolidates on its own schedule.
    #[must_use]
    pub fn scheduled() -> Self {
        Self {
            status: ConsolidateStatus::Scheduled,
            jobs: Vec::new(),
            scopes: 0,
            built: None,
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
