//! The seam between the memory agent tools and the host that runs them.
//!
//! The tools are generic over a [`MemoryToolHost`]: everything they need that is
//! not a pure function of their arguments comes through it. That is the guarded
//! memory driver for this call, the per-turn source allowlist, the embedding
//! model for query vectors, and the one write path (`ingest_document`) that
//! lives in the host's own tree-ingest code.

use std::sync::Arc;

use async_trait::async_trait;
use tinytools::ToolResult;

/// Embeds a query for similarity search.
#[async_trait]
pub trait QueryEmbedder: Send + Sync {
    /// Embed one text. The error is the provider's message.
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>, String>;

    /// Stable embedding-space identity used to select stored vectors.
    fn signature(&self) -> String;
}

/// What a host supplies to run the memory agent tools.
///
/// `Clone` because the consolidated `memory_tree` dispatcher builds the
/// per-mode tools from its own host.
#[async_trait]
pub trait MemoryToolHost: Clone + Send + Sync + 'static {
    /// The **guarded** memory driver for this call.
    ///
    /// Guarded, because the host's policy (tier, source scope, taint, budgets)
    /// runs inside it; every retrieval the tools make passes `None` for its
    /// scope and relies on that. The error is a caller-facing message; the tools
    /// prefix it with their name.
    async fn provider(&self) -> Result<Arc<dyn MemoryProvider>, String>;

    /// Whether a chunk carrying `tags` from `source_id` may be read under the
    /// ambient per-turn source allowlist. Chunks outside any memory source
    /// always pass.
    fn chunk_source_allowed(&self, tags: &[String], source_id: &str) -> bool;

    /// The embedding model for query vectors. The error is the host's full
    /// message (for example `load config failed: ...`).
    async fn embedder(&self) -> Result<Box<dyn QueryEmbedder>, String>;

    /// The `memory_tree` tool's `ingest_document` mode: write a document into
    /// the tree. The host owns the write (its config, workspace and RPC layer).
    async fn ingest_document(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;
}
