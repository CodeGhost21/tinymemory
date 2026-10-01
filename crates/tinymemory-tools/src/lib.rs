//! `tinymemory-tools` — the memory agent tools: retrieval over the summary tree,
//! raw chunk and entity search, hybrid and vector search, and tool-scoped
//! memory rules.
//!
//! Every tool is a [`tinytools::Tool`] generic over a [`MemoryToolHost`]. The
//! host supplies the guarded [`tinymemory_api::provider::MemoryProvider`] for a
//! call, the per-turn source allowlist and the query embedder; the tools own
//! their names, schemas, argument validation and output shapes, which are wire
//! contracts with the model and with saved transcripts.
//!
//! What is deliberately not here: the tools that mutate memory under the host's
//! security policy (`memory_store`, `memory_forget`, the consolidated `memory`
//! tool), the goals tool (its validation is host policy by the goals family's
//! own contract), and the tree `ingest_document` write, which reaches the
//! host's ingest path through [`MemoryToolHost::ingest_document`].

pub mod host;
pub mod query;
pub mod raw_store;
pub mod requests;
pub mod search;
pub mod tool_memory;

pub use host::{MemoryToolHost, QueryEmbedder};

#[cfg(test)]
mod test_host;

/// What a tool says when the bound driver serves no tool-memory family. Shared
/// by the `memory_tools_*` tools and the host's RPC handlers for the same
/// calls.
pub const NO_TOOL_MEMORY: &str = "memory driver does not support the tool_memory family";
