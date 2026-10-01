//! `MemoryRetrieval` over the hosted wire.
//!
//! # Scores are ranks
//!
//! The engine ranks recall but returns no score, and its recall API offers none
//! to ask for. So a hit's score here is its place in the engine's ranking: the
//! best hit scores 1.0 and each later one 0.1 less, down to 0.1. That says how
//! the engine ordered the hits, not how relevant any of them is. It fills
//! `score` and `final_score` only: no similarity was measured, so a namespace
//! hit's `vector_similarity` stays 0, and a host that floors on similarity
//! reads these hits as carrying no similarity evidence rather than as close
//! matches. Namespace recall answers only the engine's first [`RANKED_NOTES`]
//! hits: a rank says nothing about whether the tail is relevant at all, so the
//! tail is never offered.
//!
//! # A tree with only leaves
//!
//! The embedded engine retrieves over a summary tree. Hosted memory has none
//! the host can walk: each synced item is one record, a leaf with no parent.
//!
//! - `fast_retrieve` and `retrieve_source` recall across the source namespaces,
//!   or one kind's, and answer leaves.
//! - `cover_window` recalls what was observed in a window.
//! - `retrieve_leaves` reads events by id.
//! - `retrieve_children` has nothing to walk and answers empty, which is what
//!   the contract says of a node a driver does not know.
//! - `search_entities` refuses an unknown kind and is otherwise not served:
//!   the hosted service exposes no entity index.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};
use tinymemory_api::capabilities::Capability;
use tinymemory_api::chrono::{DateTime, Duration, Utc};
use tinymemory_api::chunks::SourceKind;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::types::SourceScope;
use tinymemory_api::provider::{
    CoverWindowQuery, EntityMatch, FastRetrieveQuery, MemoryRetrieval, RetrievalHit,
    RetrievalNodeKind, RetrievalResponse, SourceRetrievalQuery,
};
use tinymemory_api::types::{MemoryItemKind, NamespaceMemoryHit, RetrievalScoreBreakdown};

use super::records::{Place, Records, Version};
use super::relevance::freshness;
use super::sources::{namespace_of as source_namespace, SOURCES};
use crate::common::Attempts;
use crate::cortex::{CortexDialect, Route};
use crate::cortex_labels;
use crate::cortex_provider::CortexProvider;

/// The most hits namespace recall answers: the engine's first few, as ranked.
pub(super) const RANKED_NOTES: usize = 3;

/// The most events one recall asks for, however large the caller's limit.
const MAX_FETCH: usize = 200;

/// The entity kinds a caller may filter on, as the embedded engine names them.
const ENTITY_KINDS: [&str; 15] = [
    "email",
    "url",
    "handle",
    "hashtag",
    "person",
    "organization",
    "location",
    "event",
    "product",
    "datetime",
    "technology",
    "artifact",
    "quantity",
    "misc",
    "topic",
];

/// The score of the hit the engine ranked at `position`: 1.0 first, 0.1 less
/// for each after it, never below 0.1.
pub(super) fn rank_score(position: usize) -> f64 {
    (1.0 - 0.1 * position as f64).max(0.1)
}

/// How many events a recall asks for to answer `limit` hits: a margin over the
/// limit, because records are filtered after the engine ranks them.
fn fetch_for(limit: usize) -> usize {
    limit.saturating_mul(3).clamp(10, MAX_FETCH)
}

/// The time an RFC 3339 stamp names, or the epoch.
fn datetime(stamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(stamp)
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_default()
}

/// Seconds since the epoch of an RFC 3339 stamp, or 0.
fn seconds(stamp: &str) -> f64 {
    DateTime::parse_from_rfc3339(stamp)
        .map(|at| at.timestamp_millis() as f64 / 1000.0)
        .unwrap_or_default()
}

/// `[now − days, now + a day)` as the engine's observed-time window. The extra
/// day keeps a clock running slightly behind the engine's from cutting off what
/// was just written.
fn recent_window(days: u32) -> Value {
    let now = Utc::now();
    let since = now - Duration::days(i64::from(days));
    let until = now + Duration::days(1);
    json!({ "valid_during": [since.to_rfc3339(), until.to_rfc3339()] })
}

/// The newest recalled version of each key, in the order the engine ranked
/// its first appearance. Tombstones and empty records are dropped.
fn ranked(events: &[Value]) -> Vec<Version> {
    let mut order: Vec<String> = Vec::new();
    let mut newest: HashMap<String, Version> = HashMap::new();
    for version in events.iter().filter_map(Version::of) {
        let key = version.record.key.clone();
        if !newest.contains_key(&key) {
            order.push(key.clone());
        }
        let replace = newest
            .get(&key)
            .is_none_or(|held| version.order >= held.order);
        if replace {
            newest.insert(key, version);
        }
    }
    order
        .into_iter()
        .filter_map(|key| newest.remove(&key))
        .filter(|version| !version.deleted && !version.record.content.trim().is_empty())
        .collect()
}

/// Whether a caller restricted to `scope` may see a record synced from
/// `source`. A restricted caller never sees a record whose source is unknown.
fn visible(scope: Option<&SourceScope>, source: Option<&str>) -> bool {
    match (scope, source) {
        (None, _) => true,
        (Some(scope), Some(source)) => scope.allows_source_id(source),
        (Some(_), None) => false,
    }
}

/// One synced record as a leaf, scored `score`.
fn leaf(version: &Version, score: f64) -> RetrievalHit {
    let namespace =
        CortexDialect::namespace_of(&version.scope).unwrap_or_else(|| version.scope.clone());
    let at = datetime(
        version
            .observed_at
            .as_deref()
            .unwrap_or(&version.recorded_at),
    );
    let tree_kind = if namespace == source_namespace(SourceKind::Chat) {
        "chat"
    } else {
        "source"
    };
    RetrievalHit {
        node_id: version.event_id.clone(),
        node_kind: RetrievalNodeKind::Leaf,
        tree_scope: version
            .record
            .provenance
            .source
            .clone()
            .unwrap_or_else(|| namespace.clone()),
        tree_id: namespace,
        tree_kind: Some(tree_kind.to_string()),
        level: 0,
        content: version.record.content.clone(),
        entities: Vec::new(),
        topics: Vec::new(),
        time_range_start: at,
        time_range_end: at,
        score: score as f32,
        child_ids: Vec::new(),
        source_ref: version.record.provenance.reference.clone(),
    }
}

/// `hits` cut to `limit`, saying how many there were.
fn response(mut hits: Vec<RetrievalHit>, limit: usize) -> RetrievalResponse {
    let total = hits.len();
    hits.truncate(limit);
    RetrievalResponse {
        truncated: total > hits.len(),
        total,
        hits,
    }
}

/// One namespace hit scored `score`, which is a rank or a recency and never a
/// similarity, so the similarity signal stays 0.
fn namespace_hit(namespace: &str, version: &Version, score: f64, fresh: f64) -> NamespaceMemoryHit {
    let record = &version.record;
    let document_id = record.provenance.document.clone();
    NamespaceMemoryHit {
        id: document_id
            .clone()
            .unwrap_or_else(|| format!("kv:{namespace}:{}", record.key)),
        kind: if document_id.is_some() {
            MemoryItemKind::Document
        } else {
            MemoryItemKind::Kv
        },
        namespace: namespace.to_string(),
        key: record.key.clone(),
        title: None,
        content: record.content.clone(),
        category: record.category.to_string(),
        source_type: None,
        updated_at: seconds(&version.recorded_at),
        score,
        score_breakdown: RetrievalScoreBreakdown {
            freshness: fresh,
            final_score: score,
            ..RetrievalScoreBreakdown::default()
        },
        document_id,
        chunk_id: None,
        supporting_relations: Vec::new(),
        taint: record.taint,
    }
}

impl CortexProvider {
    /// One recall, answering its events layer.
    pub(super) async fn recall_events(&self, body: &Value) -> Result<Vec<Value>, MemoryError> {
        let answer: Value = self
            .dialect
            .client
            .json(
                Method::POST,
                self.dialect.wire.path(Route::Recall),
                Some(body),
                Attempts::RetryTransient,
            )
            .await
            .map_err(engine_error)?;
        Ok(answer
            .pointer("/layers/events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Recall across the synced records — every source namespace, or one
    /// kind's — as leaves in the engine's order.
    async fn source_leaves(
        &self,
        query: Option<&str>,
        kind: Option<SourceKind>,
        source_id: Option<&str>,
        temporal: Option<Value>,
        fetch: usize,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<RetrievalHit>, MemoryError> {
        let namespace = kind.map_or(SOURCES, source_namespace);
        let place = Place::family_namespace(&self.dialect, namespace).map_err(engine_error)?;
        let mut body = json!({
            "scope": place.scope,
            "view": "descend",
            "include": ["events"],
            "budgets": { "per_layer_limits": { "events": fetch } },
        });
        if let Some(query) = query {
            body["query"] = json!(query);
        }
        if let Some(temporal) = temporal {
            body["temporal"] = temporal;
        }
        if let Some(source_id) = source_id {
            body["filters"] =
                json!({ "metadata": { "labels": [cortex_labels::source(source_id)] } });
        }
        let leaves = ranked(&self.recall_events(&body).await?)
            .into_iter()
            .filter(|version| {
                source_id.is_none_or(|id| version.record.provenance.source.as_deref() == Some(id))
            })
            .filter(|version| visible(scope, version.record.provenance.source.as_deref()))
            .enumerate()
            .map(|(position, version)| leaf(&version, rank_score(position)))
            .collect();
        Ok(leaves)
    }
}

#[async_trait]
impl MemoryRetrieval for CortexProvider {
    async fn fast_retrieve(
        &self,
        query: &str,
        options: FastRetrieveQuery,
        scope: Option<&SourceScope>,
    ) -> Result<RetrievalResponse, MemoryError> {
        if query.trim().is_empty() {
            return Err(MemoryError::Invalid("query must not be empty".to_string()));
        }
        if options.limit == 0 {
            return Ok(RetrievalResponse::default());
        }
        let hits = self
            .source_leaves(
                Some(query),
                None,
                None,
                options.time_window_days.map(recent_window),
                fetch_for(options.limit),
                scope,
            )
            .await?;
        Ok(response(hits, options.limit))
    }

    async fn cover_window(
        &self,
        window: &CoverWindowQuery,
        scope: Option<&SourceScope>,
    ) -> Result<RetrievalResponse, MemoryError> {
        let (Some(since), Some(until)) = (
            DateTime::<Utc>::from_timestamp_millis(window.since_ms),
            DateTime::<Utc>::from_timestamp_millis(window.until_ms),
        ) else {
            return Err(MemoryError::Invalid(
                "cover_window bounds are outside the supported time range".to_string(),
            ));
        };
        if until <= since {
            return Ok(RetrievalResponse::default());
        }
        let limit = window.limit.filter(|limit| *limit > 0).unwrap_or(50);
        let mut hits = self
            .source_leaves(
                None,
                window.source_kind,
                window.source_id.as_deref(),
                Some(json!({ "valid_during": [since.to_rfc3339(), until.to_rfc3339()] })),
                limit.clamp(1, MAX_FETCH),
                scope,
            )
            .await?;
        // A window reads in time order.
        hits.sort_by_key(|hit| hit.time_range_start);
        Ok(response(hits, limit))
    }

    async fn retrieve_source(
        &self,
        query: &SourceRetrievalQuery,
        scope: Option<&SourceScope>,
    ) -> Result<RetrievalResponse, MemoryError> {
        if query.limit == 0 {
            return Ok(RetrievalResponse::default());
        }
        let text = query.query.as_deref().filter(|q| !q.trim().is_empty());
        let mut hits = self
            .source_leaves(
                text,
                query.source_kind,
                query.source_id.as_deref(),
                query.time_window_days.map(recent_window),
                fetch_for(query.limit),
                scope,
            )
            .await?;
        if text.is_none() {
            // Without a query the newest records come first.
            hits.sort_by_key(|hit| std::cmp::Reverse(hit.time_range_start));
        }
        Ok(response(hits, query.limit))
    }

    async fn retrieve_children(
        &self,
        _node_id: &str,
        _max_depth: u32,
        _query: Option<&str>,
        _limit: Option<usize>,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<RetrievalHit>, MemoryError> {
        Ok(Vec::new())
    }

    async fn retrieve_leaves(
        &self,
        chunk_ids: &[String],
        scope: Option<&SourceScope>,
    ) -> Result<Vec<RetrievalHit>, MemoryError> {
        let mut hits = Vec::new();
        let mut seen = HashSet::new();
        for id in chunk_ids {
            if !seen.insert(id.as_str()) {
                continue;
            }
            let Some(event) = self.dialect.event_by_id(id).await.map_err(engine_error)? else {
                continue;
            };
            let Some(version) = Version::of(&event) else {
                continue;
            };
            // Only an event in one of this account's scopes is a leaf here.
            if version.scope.is_empty() || version.deleted {
                continue;
            }
            if visible(scope, version.record.provenance.source.as_deref()) {
                hits.push(leaf(&version, 1.0));
            }
        }
        Ok(hits)
    }

    async fn recall_namespace_scored(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
        exclude_session_id: Option<&str>,
    ) -> Result<Vec<NamespaceMemoryHit>, MemoryError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let place = Place::namespace(&self.dialect, namespace)
            .map_err(|error| MemoryError::Invalid(format!("{error:#}")))?;
        let events = self
            .recall_events(&json!({
                "scope": place.scope,
                "query": query,
                "include": ["events"],
                "budgets": { "per_layer_limits": { "events": fetch_for(limit) } },
            }))
            .await?;
        let excluded = exclude_session_id.map(str::trim).filter(|s| !s.is_empty());
        let now = Utc::now().timestamp_millis() as f64 / 1000.0;
        Ok(ranked(&events)
            .into_iter()
            .filter(|version| {
                excluded.is_none_or(|session| version.record.session_id.as_deref() != Some(session))
            })
            .take(limit.min(RANKED_NOTES))
            .enumerate()
            .map(|(position, version)| {
                let fresh = freshness(now - seconds(&version.recorded_at));
                namespace_hit(namespace, &version, rank_score(position), fresh)
            })
            .collect())
    }

    async fn recall_namespace_recent(
        &self,
        namespace: &str,
        limit: usize,
    ) -> Result<Vec<NamespaceMemoryHit>, MemoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let place = Place::namespace(&self.dialect, namespace)
            .map_err(|error| MemoryError::Invalid(format!("{error:#}")))?;
        let now = Utc::now().timestamp_millis() as f64 / 1000.0;
        let mut live: Vec<Version> = Records::new(&self.dialect)
            .live_all(&place)
            .await
            .map_err(engine_error)?
            .into_iter()
            .filter(|version| !version.record.content.trim().is_empty())
            .collect();
        live.sort_by_key(|version| std::cmp::Reverse(version.order));
        live.truncate(limit);
        Ok(live
            .iter()
            .map(|version| {
                let fresh = freshness(now - seconds(&version.recorded_at));
                namespace_hit(namespace, version, fresh, fresh)
            })
            .collect())
    }

    async fn search_entities(
        &self,
        _query: &str,
        kinds: Option<&[String]>,
        _limit: usize,
    ) -> Result<Vec<EntityMatch>, MemoryError> {
        if let Some(unknown) = kinds
            .into_iter()
            .flatten()
            .find(|kind| !ENTITY_KINDS.contains(&kind.as_str()))
        {
            return Err(MemoryError::Invalid(format!(
                "unknown entity kind `{unknown}`"
            )));
        }
        Err(MemoryError::unsupported(Capability::Retrieval))
    }
}

#[cfg(test)]
#[path = "retrieval_test.rs"]
mod test;
