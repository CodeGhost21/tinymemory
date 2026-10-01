//! The parts of the memory tree hosted memory can serve.
//!
//! The embedded engine keeps a summary tree on the device: ingested chunks
//! sealed into hourly, daily and monthly summaries. Hosted memory seals no
//! tree; the server derives facts, beliefs and concepts from what is written,
//! on its own. So of this family:
//!
//! - **`summary_forest`** answers those layers as a forest — facts over the
//!   events they cite, beliefs over facts, concepts over both — each node
//!   carrying its text (see [`super::understanding`]).
//! - **`recent_leaves`** answers the newest notes, messages and documents in
//!   the namespaces the forest is drawn from, each under the fact that cites
//!   it, when one does.
//! - **`summarise`** has no model to fold with: this adapter reaches none, so
//!   it is `Unsupported` for anything to fold — never an empty summary, which
//!   a caller would keep as a real one.
//! - **`root_summaries_with_caps`** is empty: the server's understanding is
//!   not a root summary per namespace. **`flush_source_tree`** seals nothing,
//!   and **`flavour_profile`** has no persona to compile.
//! - The members that write or walk the tree, and the runtime namespace trees,
//!   are `Unsupported`.

use async_trait::async_trait;
use tinymemory_api::capabilities::Capability;
use tinymemory_api::chunks::Chunk;
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::types::SourceScope;
use tinymemory_api::provider::{
    MemoryTree, RootSummary, SummaryContext, SummaryInput, SummaryOutput,
};
use tinymemory_api::tree::{
    leaf_preview, IngestRequest, QueryResult, SummaryForest, TreeLeaf, TreeStatus,
};

use super::records::Version;
use super::retrieval::{datetime, ranked, source_filter, visible};
use super::understanding::derived_namespaces;
use crate::cortex_provider::CortexProvider;

/// Most leaves one listing answers.
const MAX_LEAVES: usize = 200;

/// Most summaries one forest answers.
const MAX_SUMMARIES: usize = 10_000;

fn unsupported() -> MemoryError {
    MemoryError::unsupported(Capability::Tree)
}

/// One record as a tree leaf, under `parent` when a fact cites it.
fn tree_leaf(version: &Version, namespace: &str, parent: Option<&String>) -> TreeLeaf {
    let at = datetime(
        version
            .observed_at
            .as_deref()
            .unwrap_or(&version.recorded_at),
    );
    TreeLeaf {
        chunk_id: version.event_id.clone(),
        parent_summary_id: parent.cloned(),
        source_id: version
            .record
            .provenance
            .source
            .clone()
            .unwrap_or_else(|| namespace.to_string()),
        preview: leaf_preview(&version.record.content),
        time_range_start: at,
        time_range_end: at,
    }
}

#[async_trait]
impl MemoryTree for CortexProvider {
    async fn append(&self, _request: IngestRequest) -> Result<(), MemoryError> {
        Err(unsupported())
    }

    async fn query_source(
        &self,
        _namespace: &str,
        _source_id: &str,
        _limit: usize,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<Chunk>, MemoryError> {
        Err(unsupported())
    }

    async fn drill_down(
        &self,
        _namespace: &str,
        _node_id: &str,
    ) -> Result<QueryResult, MemoryError> {
        Err(unsupported())
    }

    async fn seal(&self, _namespace: &str) -> Result<TreeStatus, MemoryError> {
        Err(unsupported())
    }

    async fn cascade(&self, _namespace: &str) -> Result<TreeStatus, MemoryError> {
        Err(unsupported())
    }

    /// The server's facts, beliefs and concepts, as a forest.
    ///
    /// A derived node draws on events from any source, so a caller confined
    /// to some sources is answered none.
    async fn summary_forest(
        &self,
        limit: usize,
        scope: Option<&SourceScope>,
    ) -> Result<SummaryForest, MemoryError> {
        if scope.is_some() {
            return Ok(SummaryForest::default());
        }
        let forest = self.forest().await?;
        let limit = limit.min(MAX_SUMMARIES);
        let summaries: Vec<_> = forest.summaries.iter().take(limit).cloned().collect();
        Ok(SummaryForest {
            truncated: forest.truncated || forest.summaries.len() > summaries.len(),
            summaries,
        })
    }

    /// The newest records the caller may see, each under the fact that cites
    /// it. A source scope narrows each listing by the allowed sources' labels,
    /// so a disallowed source cannot crowd permitted ones out of the page.
    async fn recent_leaves(
        &self,
        limit: usize,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<TreeLeaf>, MemoryError> {
        if limit == 0 || scope.is_some_and(SourceScope::is_empty) {
            return Ok(Vec::new());
        }
        let limit = limit.min(MAX_LEAVES);
        let labels = scope.map(|scope| source_filter(&scope.allow));
        // Only an unconfined caller is answered the forest the parents are in.
        let forest = match scope {
            None => Some(self.forest().await?),
            Some(_) => None,
        };
        let mut leaves: Vec<TreeLeaf> = Vec::new();
        for namespace in derived_namespaces() {
            let scope_path = self.dialect.scope_for(namespace).map_err(engine_error)?;
            let events = self
                .dialect
                .newest(&scope_path, labels.as_deref(), limit)
                .await
                .map_err(engine_error)?;
            leaves.extend(
                ranked(&events)
                    .iter()
                    .filter(|version| visible(scope, version.record.provenance.source.as_deref()))
                    .map(|version| {
                        let parent = forest
                            .as_ref()
                            .and_then(|forest| forest.leaf_parents.get(&version.event_id));
                        tree_leaf(version, namespace, parent)
                    }),
            );
        }
        leaves.sort_by_key(|leaf| std::cmp::Reverse(leaf.time_range_start));
        leaves.truncate(limit);
        Ok(leaves)
    }

    /// Nothing is buffered to seal.
    async fn flush_source_tree(&self, _source_scope: &str) -> Result<u64, MemoryError> {
        Ok(0)
    }

    /// Nothing to fold is an empty summary, as the contract asks; anything
    /// else needs a model this adapter does not reach.
    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        _context: &SummaryContext,
    ) -> Result<SummaryOutput, MemoryError> {
        if inputs.iter().all(|input| input.content.trim().is_empty()) {
            return Ok(SummaryOutput::default());
        }
        Err(unsupported())
    }

    /// There are no root summaries: the server's understanding is not one
    /// summary per namespace.
    async fn root_summaries_with_caps(
        &self,
        _per_namespace_cap: usize,
        _total_cap: usize,
    ) -> Result<Vec<RootSummary>, MemoryError> {
        Ok(Vec::new())
    }

    async fn flavour_profile(&self, scope: &str) -> Result<Option<String>, MemoryError> {
        if scope.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "flavour scope must not be empty".to_string(),
            ));
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "tree_test.rs"]
mod test;
