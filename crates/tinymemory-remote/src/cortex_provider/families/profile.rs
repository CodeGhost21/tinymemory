//! The learned profile: facets the host derives from conversations and
//! connected accounts, persisted as the host computes them.
//!
//! Each facet is one inert record in the bookkeeping scope `tmi:profile` (see
//! [`super::scopes`]), keyed by the facet's key and holding the facet as JSON
//! the engine does not learn from. The host owns stability and state; this
//! module stores them and never computes them. The one merge that is the
//! driver's — a provider observation — follows the embedded engine: a weaker
//! re-observation adds evidence without overwriting a stronger value.
//!
//! Listings are filtered and ordered here, since a profile is small enough to
//! read whole.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::{FacetState, FacetType, MemoryProfile, ProfileFacet, UserState};

use super::records::{Place, Record, Records};
use super::scopes::PROFILE;
use crate::cortex_provider::CortexProvider;

/// The class a new provider facet gets, as the embedded engine infers it: from
/// a known key prefix, then from the legacy `skill:` form, then from its type.
fn class_of(key: &str, facet_type: FacetType) -> String {
    if let Some((prefix, _)) = key.split_once('/') {
        if matches!(
            prefix,
            "style" | "identity" | "tooling" | "veto" | "goal" | "channel"
        ) {
            return prefix.to_string();
        }
    }
    if key.starts_with("skill:") {
        return "tooling".to_string();
    }
    match facet_type {
        FacetType::Role | FacetType::Personality | FacetType::Context => "identity",
        FacetType::Workflow => "tooling",
        FacetType::Preference => "style",
    }
    .to_string()
}

/// `existing` with `segment` added, comma-separated and without repeats.
fn merged_segments(existing: Option<&str>, segment: Option<&str>) -> String {
    match (existing.filter(|s| !s.is_empty()), segment) {
        (Some(existing), Some(segment)) if existing.split(',').any(|s| s == segment) => {
            existing.to_string()
        }
        (Some(existing), Some(segment)) => format!("{existing},{segment}"),
        (Some(existing), None) => existing.to_string(),
        (None, Some(segment)) => segment.to_string(),
        (None, None) => String::new(),
    }
}

/// SQL `LIKE` as SQLite applies it: `%` matches any run of characters, `_`
/// exactly one, and ASCII letters match regardless of case.
pub(super) fn like(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let text: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
    // `matches[j]`: whether the pattern so far matches the first `j` characters.
    let mut matches = vec![false; text.len() + 1];
    matches[0] = true;
    for token in &pattern {
        let mut next = vec![false; text.len() + 1];
        match token {
            '%' => {
                let mut reached = false;
                for (j, slot) in next.iter_mut().enumerate() {
                    reached |= matches[j];
                    *slot = reached;
                }
            }
            token => {
                for (j, slot) in next.iter_mut().enumerate().skip(1) {
                    *slot = matches[j - 1] && (*token == '_' || *token == text[j - 1]);
                }
            }
        }
        matches = next;
    }
    matches[text.len()]
}

impl CortexProvider {
    fn profile_place(&self) -> Result<Place, MemoryError> {
        Place::bookkeeping(&self.dialect, PROFILE.to_string()).map_err(engine_error)
    }

    /// Every facet, in no particular order. One that does not parse is
    /// skipped rather than failing the listing.
    async fn facets(&self) -> Result<Vec<ProfileFacet>, MemoryError> {
        let place = self.profile_place()?;
        Ok(Records::new(&self.dialect)
            .live_all(&place)
            .await
            .map_err(engine_error)?
            .into_iter()
            .filter_map(|live| serde_json::from_str(&live.record.content).ok())
            .collect())
    }

    async fn write_facet(&self, facet: &ProfileFacet) -> Result<(), MemoryError> {
        if facet.key.trim().is_empty() {
            return Err(MemoryError::Invalid("a facet needs a key".to_string()));
        }
        let place = self.profile_place()?;
        let record = Record::plain(facet.key.clone(), serde_json::to_string(facet)?);
        Records::new(&self.dialect)
            .put(&place, &record, None)
            .await
            .map_err(engine_error)
    }

    async fn remove_facet(&self, key: &str) -> Result<bool, MemoryError> {
        let place = self.profile_place()?;
        Records::new(&self.dialect)
            .remove(&place, key)
            .await
            .map_err(engine_error)
    }
}

/// Most stable first, then by key so equal stability reads the same each time.
fn by_stability(facets: &mut [ProfileFacet]) {
    facets.sort_by(|a, b| {
        b.stability
            .total_cmp(&a.stability)
            .then_with(|| a.key.cmp(&b.key))
    });
}

#[async_trait]
impl MemoryProfile for CortexProvider {
    async fn list_active_facets(&self) -> Result<Vec<ProfileFacet>, MemoryError> {
        let mut facets: Vec<ProfileFacet> = self
            .facets()
            .await?
            .into_iter()
            .filter(|facet| facet.state == FacetState::Active)
            .collect();
        by_stability(&mut facets);
        Ok(facets)
    }

    async fn list_all_facets(&self) -> Result<Vec<ProfileFacet>, MemoryError> {
        let mut facets = self.facets().await?;
        by_stability(&mut facets);
        Ok(facets)
    }

    async fn get_facet(&self, key: &str) -> Result<Option<ProfileFacet>, MemoryError> {
        let place = self.profile_place()?;
        Ok(Records::new(&self.dialect)
            .live(&place, key)
            .await
            .map_err(engine_error)?
            .and_then(|live| serde_json::from_str(&live.record.content).ok()))
    }

    async fn facets_by_type(
        &self,
        facet_type: FacetType,
    ) -> Result<Vec<ProfileFacet>, MemoryError> {
        let mut facets: Vec<ProfileFacet> = self
            .facets()
            .await?
            .into_iter()
            .filter(|facet| facet.facet_type == facet_type)
            .collect();
        facets.sort_by(|a, b| {
            b.evidence_count
                .cmp(&a.evidence_count)
                .then_with(|| a.key.cmp(&b.key))
        });
        Ok(facets)
    }

    async fn upsert_facet(&self, facet: &ProfileFacet) -> Result<(), MemoryError> {
        self.write_facet(facet).await
    }

    async fn upsert_provider_facet(
        &self,
        facet_id: &str,
        facet_type: FacetType,
        key: &str,
        value: &str,
        confidence: f64,
        segment_id: Option<&str>,
        observed_at: f64,
    ) -> Result<(), MemoryError> {
        let existing = self
            .get_facet(key)
            .await?
            .filter(|facet| facet.facet_type == facet_type);
        let facet = match existing {
            Some(mut facet) => {
                facet.evidence_count = facet.evidence_count.saturating_add(1);
                facet.source_segment_ids = Some(merged_segments(
                    facet.source_segment_ids.as_deref(),
                    segment_id,
                ));
                facet.last_seen_at = observed_at;
                if confidence >= facet.confidence {
                    facet.value = value.to_string();
                    facet.confidence = confidence;
                }
                facet
            }
            None => ProfileFacet {
                facet_id: facet_id.to_string(),
                facet_type,
                key: key.to_string(),
                value: value.to_string(),
                confidence,
                evidence_count: 1,
                source_segment_ids: Some(segment_id.unwrap_or_default().to_string()),
                first_seen_at: observed_at,
                last_seen_at: observed_at,
                state: FacetState::Active,
                stability: 0.0,
                user_state: UserState::Auto,
                evidence_refs: Vec::new(),
                class: Some(class_of(key, facet_type)),
                cue_families: None,
            },
        };
        self.write_facet(&facet).await
    }

    async fn set_facet_user_state(
        &self,
        key: &str,
        user_state: UserState,
    ) -> Result<bool, MemoryError> {
        let Some(mut facet) = self.get_facet(key).await? else {
            return Ok(false);
        };
        facet.user_state = user_state;
        self.write_facet(&facet).await?;
        Ok(true)
    }

    async fn delete_facet(&self, key: &str) -> Result<bool, MemoryError> {
        self.remove_facet(key).await
    }

    async fn delete_facet_by_id(&self, facet_id: &str) -> Result<bool, MemoryError> {
        let Some(key) = self
            .facets()
            .await?
            .into_iter()
            .find(|facet| facet.facet_id == facet_id)
            .map(|facet| facet.key)
        else {
            return Ok(false);
        };
        self.remove_facet(&key).await
    }

    async fn drop_facets_below(&self, threshold: f64) -> Result<usize, MemoryError> {
        let doomed: Vec<String> = self
            .facets()
            .await?
            .into_iter()
            .filter(|facet| {
                facet.state == FacetState::Dropped
                    && facet.stability < threshold
                    && facet.user_state != UserState::Pinned
            })
            .map(|facet| facet.key)
            .collect();
        let mut dropped = 0;
        for key in doomed {
            if self.remove_facet(&key).await? {
                dropped += 1;
            }
        }
        Ok(dropped)
    }

    async fn workflow_identity_matches(&self, key_pattern: &str, canonical_value: &str) -> bool {
        // The contract reads any failure as "no match".
        self.facets().await.is_ok_and(|facets| {
            facets.iter().any(|facet| {
                facet.facet_type == FacetType::Workflow
                    && facet.value == canonical_value
                    && like(key_pattern, &facet.key)
            })
        })
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod test;
