//! The reference engine's consolidation: a toy belief per source item.
//!
//! Real engines distil beliefs with a model. The reference engine needs only
//! something deterministic and obvious: each document or conversation an
//! [`ConsolidateRequest`] admits yields one [`LearningKind::Fact`] — the
//! first sentence of the document's body, or of the conversation's first user
//! turn — stored at the source item's own node with the source's metadata, a
//! `consolidated` tag, and the source's id as evidence. Consolidating twice is
//! a replay, so it never duplicates a belief.

use crate::{ConsolidateRequest, DocumentBody, ItemKind, LearningKind, Role, StoreItem};

/// Tag on every belief the reference engine distils.
pub const CONSOLIDATED_TAG: &str = "consolidated";

/// Confidence of a distilled belief: a first sentence is a weak signal.
const BELIEF_CONFIDENCE: f32 = 0.5;

/// Longest statement a belief keeps, in characters.
const MAX_STATEMENT_CHARS: usize = 240;

/// The beliefs `request` distils from `items`, in item order.
pub(super) fn distil(items: &[StoreItem], request: &ConsolidateRequest) -> Vec<StoreItem> {
    let kinds = request.admitted_kinds();
    items
        .iter()
        .filter(|item| item.kind() != ItemKind::Learning && kinds.contains(&item.kind()))
        .filter(|item| request.reach.admits(&item.meta().namespace))
        .filter_map(|item| {
            let statement = first_sentence(&source_text(item)?)?;
            let mut meta = item.meta().clone();
            if !meta.tags.iter().any(|tag| tag == CONSOLIDATED_TAG) {
                meta.tags.push(CONSOLIDATED_TAG.to_string());
            }
            Some(StoreItem::Learning {
                text: statement,
                kind: LearningKind::Fact,
                confidence: BELIEF_CONFIDENCE,
                evidence: Some(item.fingerprint()),
                meta,
            })
        })
        .collect()
}

/// The text a belief is drawn from: a document's body, or a conversation's
/// first user turn.
fn source_text(item: &StoreItem) -> Option<String> {
    match item {
        StoreItem::Document {
            body: DocumentBody::Text(text),
            ..
        } => Some(text.clone()),
        StoreItem::Conversation { turns, .. } => turns
            .iter()
            .find(|turn| turn.role == Role::User)
            .map(|turn| turn.text.clone()),
        _ => None,
    }
}

/// The first sentence of `text`, markdown heading markers stripped and
/// whitespace collapsed; `None` when nothing is left.
fn first_sentence(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())?;
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = collapsed
        .char_indices()
        .find(|(_, c)| matches!(c, '.' | '!' | '?'))
        .map_or(collapsed.len(), |(index, c)| index + c.len_utf8());
    let sentence: String = collapsed[..end].chars().take(MAX_STATEMENT_CHARS).collect();
    (!sentence.is_empty()).then_some(sentence)
}

#[cfg(test)]
#[path = "distil_tests.rs"]
mod tests;
