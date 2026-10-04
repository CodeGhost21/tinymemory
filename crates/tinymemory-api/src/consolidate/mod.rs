//! Consolidation: asking an engine to distil what it holds into beliefs.
//!
//! Engines that keep a cognitive layer (CortexDB's beliefs and facts) build
//! it from raw events — documents, conversation turns — with a model, which
//! takes seconds to minutes. That work never belongs on an agent's live turn,
//! so the contract only lets a host *ask* for it:
//! [`MemoryEngine::consolidate`](crate::MemoryEngine::consolidate) names a
//! [`ConsolidateRequest`] (which part of the tree, which kinds) and returns as
//! soon as the engine has taken the job. What it builds comes back through
//! ordinary reads.
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
