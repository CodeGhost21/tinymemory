//! Test-only knobs for the hosted families.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use super::maintenance::ProbeCache;
use super::sources::Pacing;
use super::understanding::{ForestCache, FOREST_TTL};
use super::FamilyState;
use crate::cortex_provider::CortexProvider;
use crate::hosted_test_support::{hosted_backend, provider_with_budget, Shared};

impl FamilyState {
    /// State with a pacing gap and probe reuse windows of the test's choosing,
    /// so a test need not wait out the real ones.
    pub(crate) fn with(pacing: Duration, healthy_for: Duration, failed_for: Duration) -> Self {
        Self {
            pacing: Pacing::new(pacing),
            probe: ProbeCache::new(healthy_for, failed_for),
            forest: ForestCache::new(FOREST_TTL),
        }
    }
}

/// A hosted provider over a fresh `/memory/*` double, with no pacing, probe
/// answers reused for an hour, and a five-second visibility budget.
pub(crate) async fn hosted() -> (CortexProvider, Shared) {
    let (endpoint, state) = hosted_backend().await;
    let provider = provider_with_budget(&endpoint, Duration::from_secs(5)).with_families(
        FamilyState::with(Duration::ZERO, Duration::from_secs(3600), Duration::ZERO),
    );
    (provider, state)
}

/// The requests the double has seen, as `METHOD path?query` strings.
pub(crate) fn requests(state: &Shared) -> Vec<String> {
    state.seen.lock().expect("seen").requests.clone()
}

/// The recall bodies the double has seen, in the order they arrived.
pub(crate) fn recalls(state: &Shared) -> Vec<serde_json::Value> {
    state.seen.lock().expect("seen").recalls.clone()
}
