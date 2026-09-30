//! Argument shapes shared by the memory-tree retrieval tools and, in the host,
//! the JSON-RPC handlers that answer the same five calls.

use serde::{Deserialize, Serialize};

/// Request body for `memory_tree_query_source`. All fields are optional; see
/// `MemoryRetrieval::retrieve_source` for selection semantics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuerySourceRequest {
    /// Exact source id.
    #[serde(default)]
    pub source_id: Option<String>,
    /// Source kind filter when no exact id is known.
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Only summaries whose time range overlaps the last N days.
    #[serde(default)]
    pub time_window_days: Option<u32>,
    /// Phase 4 (#710) — optional natural-language query string. When
    /// provided, candidates are reranked by cosine similarity to the
    /// query's embedding rather than sorted by recency. Legacy rows
    /// with no stored embedding fall to the bottom.
    #[serde(default)]
    pub query: Option<String>,
    /// Max hits.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Request body for `memory_tree_cover_window`. `since_ms`/`until_ms` are the
/// inclusive window bounds in epoch-milliseconds; the source filter mirrors
/// `query_source`. See `MemoryRetrieval::cover_window` for cover semantics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CoverWindowRequest {
    /// Inclusive window start, epoch-milliseconds.
    pub since_ms: i64,
    /// Inclusive window end, epoch-milliseconds.
    pub until_ms: i64,
    /// Exact source id.
    #[serde(default)]
    pub source_id: Option<String>,
    /// Source kind filter when no exact id is known.
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Max hits.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Request body for `memory_tree_search_entities`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchEntitiesRequest {
    /// Substring to match.
    pub query: String,
    /// Optional entity-kind filter.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Max matches.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Request body for `memory_tree_drill_down`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrillDownRequest {
    /// Summary node to expand.
    pub node_id: String,
    /// How many levels to expand.
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// When set, visited children are reranked by cosine similarity between
    /// the query embedding and each child's stored embedding. Legacy children
    /// without an embedding sort to the bottom.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional cap on the returned hit count, applied AFTER rerank so the
    /// top-K is relevance-based when `query` is provided.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Request body for `memory_tree_fetch_leaves`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchLeavesRequest {
    /// Chunk ids to hydrate.
    pub chunk_ids: Vec<String>,
}
