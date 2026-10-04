//! `context.md`: a token-budgeted brief compiled from any TinyMemory engine,
//! for a host to inject at the start of a session.
//!
//! A [`ContextSpec`] names the budget, the [`Brief`]s (each a heading and the
//! question that fills it) and how many learnings to list. [`compile`] (or a
//! [`ContextCompiler`]) recalls each brief, lists the stored learnings newest
//! and most confident first, and renders a markdown document with frontmatter
//! recording `generated_at`, `engine`, `tokens` and `refs`.
//!
//! The document is `---` frontmatter, a `# Context` heading, one `## <heading>`
//! section per brief that cited something, and a `## Learnings` bullet list.
//! `tokens` in the frontmatter is the document's own estimate, frontmatter
//! included, and `refs` lists every item the document cites in order of first
//! citation. [`ContextSpec::reach`] confines every brief's recall and the
//! learnings listing to one agent's part of the memory tree.
//!
//! Rules, from the spec:
//!
//! - The whole document fits `budget_tokens`, estimated at four characters
//!   per token ([`estimate_tokens`]). Briefs keep their order; learnings are
//!   trimmed first (one line at a time from the end), then the last brief
//!   shrinks and is dropped once too little of it is left.
//! - An engine with nothing stored yields an empty document, not an error. So
//!   does a budget too small for anything to survive trimming.
//! - A brief that fails, or cites nothing, is skipped (a failure is logged); a
//!   failed learnings listing leaves the learnings out. Neither fails the
//!   document. The only error is an invalid spec ([`ContextSpec::validate`]).
//!
//! # Example
//!
//! ```
//! use tinymemory_api::{LearningKind, MemoryEngine, MemoryMeta, StoreItem};
//! use tinymemory_api::conformance::ReferenceEngine;
//! use tinymemory_tools::context::{ContextSpec, compile};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let engine = ReferenceEngine::new();
//! let empty = compile(&engine, &ContextSpec::default()).await?;
//! assert!(empty.markdown.is_empty());
//!
//! engine
//!     .store(StoreItem::learning("prefers short answers", LearningKind::Preference, 0.9, MemoryMeta::default()))
//!     .await
//!     .map_err(|e| tinymemory_tools::context::Error::InvalidSpec(e.to_string()))?;
//! let doc = compile(&engine, &ContextSpec::default()).await?;
//! assert!(doc.markdown.contains("## Learnings"));
//! assert!(doc.tokens <= ContextSpec::default().budget_tokens);
//! # Ok::<(), tinymemory_tools::context::Error>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod compile;
pub mod error;
pub mod spec;

pub use compile::{ContextCompiler, ContextDoc, compile, estimate_tokens};
pub use error::{Error, Result};
pub use spec::{Brief, ContextSpec, DEFAULT_BUDGET_TOKENS, DEFAULT_LEARNINGS_LIMIT};
