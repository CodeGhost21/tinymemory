//! Optional families served over the TinyHumans hosted wire.
//!
//! Specified in `docs/specs/tinyhumans-hosted-families.md`. Goals, tool rules,
//! documents, the source sink and maintenance, built on the record format the
//! adapter already writes, so a family record is readable by the mandatory
//! surface and the other way round. The Direct wire advertises none of them.
//!
//! - [`records`] is the keyed record layer every family writes through: lookup
//!   labels, provenance, inert bookkeeping, and retiring superseded versions.
//! - [`scopes`] names the bookkeeping scopes, which no namespace maps to.
//! - One module per family implements its contract trait on
//!   [`CortexProvider`](super::CortexProvider).

mod documents;
mod episodic;
mod goals;
mod maintenance;
mod profile;
mod records;
mod relevance;
mod retrieval;
mod scopes;
mod scoring;
mod sources;
mod tool_rules;
mod tree;
mod understanding;

use maintenance::{ProbeCache, FAILED_FOR, HEALTHY_FOR};
use sources::{Pacing, SOURCE_PACING};
use understanding::{ForestCache, FOREST_TTL};

/// What the families keep between calls on one provider.
#[derive(Debug)]
pub(crate) struct FamilyState {
    /// The gate synced writes queue at.
    pub(crate) pacing: Pacing,
    /// The last health-probe answer.
    pub(crate) probe: ProbeCache,
    /// The last reading of the server's derived layers.
    pub(crate) forest: ForestCache,
}

impl Default for FamilyState {
    fn default() -> Self {
        Self {
            pacing: Pacing::new(SOURCE_PACING),
            probe: ProbeCache::new(HEALTHY_FOR, FAILED_FOR),
            forest: ForestCache::new(FOREST_TTL),
        }
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "capabilities_test.rs"]
mod capabilities_test;
