//! [`ContextCompiler`]: gathers briefs and learnings from an engine and
//! renders them into a budgeted `context.md`.
//!
//! Gathering is one recall per brief, then a listing of learnings. A brief
//! whose recall fails, or that cites nothing, is skipped (a failure is logged);
//! a failed learnings listing leaves the learnings out. Neither fails the
//! document, so an engine that holds nothing yields an empty document.

mod render;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tinymemory_api::{
    Hit, ItemId, ItemKind, ListRequest, MemoryEngine, MetaFilter, Reach, RecallRequest,
};

use crate::error::Result;
use crate::spec::ContextSpec;
use render::{BriefSection, LearningLine, Sections};

pub use render::estimate_tokens;

/// Most citations one brief's recall gathers.
const BRIEF_CITATIONS: usize = 8;

/// Page size of the learnings listing.
const LEARNINGS_PAGE: usize = 100;

/// Most learnings pages read before sorting; a ceiling, not a target.
const LEARNINGS_MAX_PAGES: usize = 50;

/// Instructions sent with every brief's recall.
const BRIEF_INSTRUCTIONS: &str = "Answer briefly, as markdown bullet points suitable for a \
     context brief. State only what the stored items support.";

/// A compiled `context.md`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextDoc {
    /// The document; empty when the engine had nothing to say.
    pub markdown: String,
    /// The document's estimated tokens, four characters per token.
    pub tokens: usize,
    /// When it was compiled.
    pub generated_at: DateTime<Utc>,
    /// The id of the engine it was compiled from.
    pub engine: String,
    /// Every item the document cites, in order of first citation.
    pub refs: Vec<ItemId>,
}

/// Compiles `context.md` documents.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextCompiler {
    at: Option<DateTime<Utc>>,
}

impl ContextCompiler {
    /// A compiler stamping documents with the current time.
    #[must_use]
    pub fn new() -> Self {
        Self { at: None }
    }

    /// A compiler stamping every document with `generated_at`, for
    /// reproducible output.
    #[must_use]
    pub fn at(generated_at: DateTime<Utc>) -> Self {
        Self {
            at: Some(generated_at),
        }
    }

    /// Compiles `spec` against `engine`.
    ///
    /// # Errors
    ///
    /// [`crate::Error::InvalidSpec`] when the spec cannot produce a document.
    /// Engine failures are not errors: see the module docs.
    pub async fn compile(
        &self,
        engine: &dyn MemoryEngine,
        spec: &ContextSpec,
    ) -> Result<ContextDoc> {
        spec.validate()?;
        let generated_at = self.at.unwrap_or_else(Utc::now);
        let engine_id = engine.descriptor().id;
        let sections = Sections {
            briefs: gather_briefs(engine, spec).await,
            learnings: gather_learnings(engine, spec.learnings_limit, spec.reach.as_ref()).await,
        };
        let rendered = render::render(sections, spec.budget_tokens, engine_id, generated_at);
        log::debug!(
            "[context] compiled engine={engine_id} tokens={} refs={}",
            rendered.tokens,
            rendered.refs.len()
        );
        Ok(ContextDoc {
            markdown: rendered.markdown,
            tokens: rendered.tokens,
            generated_at,
            engine: engine_id.to_string(),
            refs: rendered.refs,
        })
    }
}

/// Compiles `spec` against `engine`, stamped with the current time.
///
/// # Errors
///
/// [`crate::Error::InvalidSpec`] when the spec cannot produce a document.
pub async fn compile(engine: &dyn MemoryEngine, spec: &ContextSpec) -> Result<ContextDoc> {
    ContextCompiler::new().compile(engine, spec).await
}

async fn gather_briefs(engine: &dyn MemoryEngine, spec: &ContextSpec) -> Vec<BriefSection> {
    let mut sections = Vec::with_capacity(spec.briefs.len());
    for brief in &spec.briefs {
        let mut filter = brief.filter.clone();
        if spec.reach.is_some() {
            filter.reach = spec.reach.clone();
        }
        let request = RecallRequest {
            question: brief.question.clone(),
            filter,
            limit: BRIEF_CITATIONS,
            instructions: Some(BRIEF_INSTRUCTIONS.to_string()),
        };
        match engine.recall(request).await {
            Ok(answer) if answer.citations.is_empty() || answer.answer.trim().is_empty() => {
                log::debug!(
                    "[context] brief skipped heading={:?} reason=nothing_cited",
                    brief.heading
                );
            }
            Ok(answer) => sections.push(BriefSection {
                heading: brief.heading.clone(),
                body: answer.answer.trim().to_string(),
                refs: answer.citations.into_iter().map(|c| c.id).collect(),
            }),
            Err(error) => {
                log::warn!(
                    "[context] brief skipped heading={:?} error={error}",
                    brief.heading
                );
            }
        }
    }
    sections
}

async fn gather_learnings(
    engine: &dyn MemoryEngine,
    limit: usize,
    reach: Option<&Reach>,
) -> Vec<LearningLine> {
    if limit == 0 {
        return Vec::new();
    }
    let mut all: Vec<Hit> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..LEARNINGS_MAX_PAGES {
        let filter = MetaFilter {
            reach: reach.cloned(),
            ..MetaFilter::kinds([ItemKind::Learning])
        };
        let mut request = ListRequest::new(filter, LEARNINGS_PAGE);
        request.cursor = cursor.take();
        match engine.list(request).await {
            Ok(page) => {
                all.extend(
                    page.items
                        .into_iter()
                        .filter(|hit| hit.kind == ItemKind::Learning),
                );
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            Err(error) => {
                log::warn!("[context] learnings skipped error={error}");
                return Vec::new();
            }
        }
    }
    rank_learnings(all)
        .into_iter()
        .take(limit)
        .map(|hit| LearningLine {
            id: hit.id,
            text: hit.text,
        })
        .collect()
}

/// Newest first, then most confident; undated learnings after dated ones,
/// and ties keep the engine's listing order.
fn rank_learnings(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by(|a, b| {
        b.meta.observed_at.cmp(&a.meta.observed_at).then_with(|| {
            b.confidence
                .unwrap_or(0.0)
                .total_cmp(&a.confidence.unwrap_or(0.0))
        })
    });
    hits
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
