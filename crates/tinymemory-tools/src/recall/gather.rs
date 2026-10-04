//! Filling one section from the engine, and turning what it found into a
//! renderable section.
//!
//! Every section runs on its own and none can fail the pack: an engine error
//! or an empty result becomes a [`SkippedSection`], logged and reported.
//! [`section`] reads; [`settle`] then applies the request's exclusions and
//! the items earlier sections already list, in section order, so an item is
//! listed once — in its highest-priority section.

use std::collections::HashSet;

use tinymemory_api::{
    FetchMode, FetchRequest, Hit, ItemId, ListRequest, MemoryEngine, MetaFilter, RecallRequest,
};

use super::render::{Body, Line, Section, shorten, single_line};
use super::types::{HolisticRecall, ScopeSection, SectionHits, SectionQuery, SkippedSection};

/// Longest a single bullet may be, in characters, before it is shortened: one
/// long document must not crowd out a section.
const MAX_LINE_CHARS: usize = 600;

/// Page size of a [`SectionQuery::Latest`] listing.
const LATEST_PAGE: usize = 100;

/// Most listing pages read before ranking; a ceiling, not a target.
const LATEST_MAX_PAGES: usize = 50;

/// What one section read.
pub(super) enum Gathered {
    /// An answer and its citations.
    Answered(Section, SectionHits),
    /// Ranked or latest hits, before exclusions.
    Hits(Vec<Hit>),
    /// Nothing, and why.
    Skipped(SkippedSection),
}

/// What one section contributes once settled.
pub(super) enum Settled {
    /// Something to render, and what it was drawn from.
    Filled(Section, SectionHits),
    /// Nothing, and why.
    Skipped(SkippedSection),
}

/// Fills `section` for `request`.
pub(super) async fn section(
    engine: &dyn MemoryEngine,
    request: &HolisticRecall,
    section: &ScopeSection,
) -> Gathered {
    let want = wanted(request, section);
    let outcome = match &section.query {
        SectionQuery::Answer {
            question,
            instructions,
            fallback_to_fetch,
        } => match answer(engine, section, question, instructions.clone()).await {
            Ok(Some(filled)) => return filled,
            Ok(None) => Ok(Vec::new()),
            Err(error) if *fallback_to_fetch => {
                log::debug!(
                    "[recall] answer failed, fetching instead heading={:?} error={error}",
                    section.heading
                );
                fetch(engine, &section.filter, question, want).await
            }
            Err(error) => Err(error),
        },
        SectionQuery::Fetch { query } => match query.as_deref().or(request.query.as_deref()) {
            Some(query) if !query.trim().is_empty() => {
                fetch(engine, &section.filter, query, want).await
            }
            _ => latest(engine, &section.filter, want).await,
        },
        SectionQuery::Latest => latest(engine, &section.filter, want).await,
    };
    match outcome {
        Ok(hits) => Gathered::Hits(hits),
        Err(error) => {
            log::warn!(
                "[recall] section skipped heading={:?} error={error}",
                section.heading
            );
            skipped(section, error.to_string())
        }
    }
}

/// How many hits to ask for so that `section.limit` survive the request's
/// exclusions and the items earlier sections already show: one more per
/// excluded id and per item an earlier section may hold, and double when a
/// whole thread window may be left out.
fn wanted(request: &HolisticRecall, section: &ScopeSection) -> usize {
    let window = if request.exclude_thread.is_some() {
        section.limit
    } else {
        0
    };
    let earlier: usize = request
        .sections
        .iter()
        .take_while(|other| !std::ptr::eq(*other, section))
        .map(|other| other.limit)
        .sum();
    section.limit + request.exclude_ids.len() + window + earlier
}

fn skipped(section: &ScopeSection, reason: String) -> Gathered {
    Gathered::Skipped(skipped_section(section, reason))
}

fn skipped_section(section: &ScopeSection, reason: String) -> SkippedSection {
    SkippedSection {
        heading: section.heading.clone(),
        reason,
    }
}

/// One recall; `None` when it cited nothing or answered blank.
async fn answer(
    engine: &dyn MemoryEngine,
    section: &ScopeSection,
    question: &str,
    instructions: Option<String>,
) -> tinymemory_api::Result<Option<Gathered>> {
    let answer = engine
        .recall(RecallRequest {
            question: question.to_string(),
            filter: section.filter.clone(),
            limit: section.limit,
            instructions,
        })
        .await?;
    if answer.citations.is_empty() || answer.answer.trim().is_empty() {
        return Ok(None);
    }
    let text = answer.answer.trim().to_string();
    let hits: Vec<Hit> = answer
        .citations
        .into_iter()
        .map(|citation| Hit {
            id: citation.id,
            kind: citation.kind,
            text: citation.snippet,
            meta: citation.meta,
            score: citation.score.unwrap_or_default(),
            confidence: None,
        })
        .collect();
    let refs = hits.iter().map(|hit| hit.id.clone()).collect();
    Ok(Some(Gathered::Answered(
        Section {
            heading: section.heading.clone(),
            body: Body::Prose {
                text: text.clone(),
                refs,
            },
        },
        SectionHits {
            heading: section.heading.clone(),
            answer: Some(text),
            hits,
        },
    )))
}

/// The fetch mode a section ranks with: hybrid when the engine serves it,
/// else the first it declares.
fn preferred_mode(engine: &dyn MemoryEngine) -> Option<FetchMode> {
    let modes = &engine.descriptor().fetch_modes;
    if modes.contains(&FetchMode::Hybrid) {
        Some(FetchMode::Hybrid)
    } else {
        modes.first().copied()
    }
}

/// One page of ranked hits; an engine that declares no fetch mode is read
/// newest first instead.
async fn fetch(
    engine: &dyn MemoryEngine,
    filter: &MetaFilter,
    query: &str,
    limit: usize,
) -> tinymemory_api::Result<Vec<Hit>> {
    let Some(mode) = preferred_mode(engine) else {
        return latest(engine, filter, limit).await;
    };
    let mut request = FetchRequest::new(query, mode, limit);
    request.filter = filter.clone();
    Ok(engine.fetch(request).await?.hits)
}

/// The newest hits, then the most confident, then the latest turn; ties
/// keep the engine's order.
async fn latest(
    engine: &dyn MemoryEngine,
    filter: &MetaFilter,
    limit: usize,
) -> tinymemory_api::Result<Vec<Hit>> {
    let mut all: Vec<Hit> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..LATEST_MAX_PAGES {
        let mut request = ListRequest::new(filter.clone(), LATEST_PAGE);
        request.cursor = cursor.take();
        let page = engine.list(request).await?;
        all.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    all.sort_by(|a, b| {
        b.meta
            .observed_at
            .cmp(&a.meta.observed_at)
            .then_with(|| {
                b.confidence
                    .unwrap_or(0.0)
                    .total_cmp(&a.confidence.unwrap_or(0.0))
            })
            .then_with(|| {
                let last = |hit: &Hit| hit.meta.turns.as_ref().map(|turns| turns.last);
                last(b).cmp(&last(a))
            })
    });
    all.truncate(limit);
    Ok(all)
}

/// Settles one gathered section: answers pass through untouched (an answer
/// citing an item does not hide its bullet elsewhere); hits lose the
/// request's exclusions and any item an earlier section already lists, are
/// cut to the section's limit, and join `shown`.
pub(super) fn settle(
    request: &HolisticRecall,
    section: &ScopeSection,
    gathered: Gathered,
    shown: &mut HashSet<ItemId>,
) -> Settled {
    let hits = match gathered {
        Gathered::Skipped(reason) => return Settled::Skipped(reason),
        Gathered::Answered(rendered, hits) => return Settled::Filled(rendered, hits),
        Gathered::Hits(hits) => hits,
    };
    let kinds = &section.filter.kinds;
    let hits: Vec<Hit> = hits
        .into_iter()
        .filter(|hit| kinds.is_empty() || kinds.contains(&hit.kind))
        .filter(|hit| !request.excludes(hit) && !shown.contains(&hit.id))
        .take(section.limit)
        .collect();
    if hits.is_empty() {
        return Settled::Skipped(skipped_section(section, "empty".to_string()));
    }
    shown.extend(hits.iter().map(|hit| hit.id.clone()));
    let lines = hits
        .iter()
        .map(|hit| {
            let text = single_line(&hit.text);
            let text = if text.chars().count() > MAX_LINE_CHARS {
                shorten(&text, MAX_LINE_CHARS)
            } else {
                text
            };
            Line {
                id: hit.id.clone(),
                text,
            }
        })
        .collect();
    Settled::Filled(
        Section {
            heading: section.heading.clone(),
            body: Body::Lines(lines),
        },
        SectionHits {
            heading: section.heading.clone(),
            answer: None,
            hits,
        },
    )
}
