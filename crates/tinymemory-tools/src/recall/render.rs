//! Rendering gathered sections into budgeted markdown.
//!
//! Pure: given the sections and the budget, the output is fixed. Trimming
//! order is the spec's: learnings go first, one line at a time from the end;
//! then the last remaining brief is shortened, and dropped once too little of
//! it is left, so earlier briefs keep their text longest and every brief keeps
//! its place.

use chrono::{DateTime, SecondsFormat, Utc};
use tinymemory_api::ItemId;

/// Characters per estimated token.
const CHARS_PER_TOKEN: usize = 4;

/// A shortened brief shorter than this is dropped rather than kept as a stub.
const MIN_BRIEF_CHARS: usize = 40;

/// Marks a shortened brief.
const ELLIPSIS: char = '…';

/// The estimated token count of `text`: four characters per token, rounded
/// up — the estimate every context budget uses.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// One answered brief.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BriefSection {
    pub(crate) heading: String,
    pub(crate) body: String,
    pub(crate) refs: Vec<ItemId>,
}

/// One learning line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LearningLine {
    pub(crate) id: ItemId,
    pub(crate) text: String,
}

/// What the document is rendered from.
#[derive(Debug, Clone)]
pub(crate) struct Sections {
    pub(crate) briefs: Vec<BriefSection>,
    pub(crate) learnings: Vec<LearningLine>,
}

/// The rendered document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rendered {
    pub(crate) markdown: String,
    pub(crate) tokens: usize,
    pub(crate) refs: Vec<ItemId>,
}

/// Renders `sections` within `budget_tokens`, trimming learnings first.
pub(crate) fn render(
    mut sections: Sections,
    budget_tokens: usize,
    engine: &str,
    generated_at: DateTime<Utc>,
) -> Rendered {
    loop {
        let rendered = compose(&sections, engine, generated_at);
        if rendered.tokens <= budget_tokens {
            return rendered;
        }
        if sections.learnings.pop().is_some() {
            continue;
        }
        let Some(last) = sections.briefs.last_mut() else {
            // Nothing left to trim: an empty document always fits.
            return compose(&sections, engine, generated_at);
        };
        let overflow_chars = (rendered.tokens - budget_tokens) * CHARS_PER_TOKEN;
        let keep = last
            .body
            .chars()
            .count()
            .saturating_sub(overflow_chars + ELLIPSIS.len_utf8());
        if keep < MIN_BRIEF_CHARS {
            sections.briefs.pop();
        } else {
            last.body = shorten(&last.body, keep);
        }
    }
}

/// The first `keep` characters of `text`, cut back to a word boundary when
/// one is near, with an ellipsis.
fn shorten(text: &str, keep: usize) -> String {
    let cut: String = text.chars().take(keep).collect();
    let trimmed = match cut.rfind(char::is_whitespace) {
        Some(space) if space * 2 > cut.len() => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}{ELLIPSIS}", trimmed.trim_end())
}

/// Composes the full document, frontmatter included, without trimming.
fn compose(sections: &Sections, engine: &str, generated_at: DateTime<Utc>) -> Rendered {
    if sections.briefs.is_empty() && sections.learnings.is_empty() {
        return Rendered {
            markdown: String::new(),
            tokens: 0,
            refs: Vec::new(),
        };
    }
    let mut refs: Vec<ItemId> = Vec::new();
    let mut body = String::from("# Context\n");
    for brief in &sections.briefs {
        body.push_str(&format!(
            "\n## {}\n\n{}\n",
            brief.heading,
            brief.body.trim()
        ));
        push_unique(&mut refs, &brief.refs);
    }
    if !sections.learnings.is_empty() {
        body.push_str("\n## Learnings\n\n");
        for line in &sections.learnings {
            body.push_str(&format!("- {}\n", single_line(&line.text)));
            push_unique(&mut refs, std::slice::from_ref(&line.id));
        }
    }
    // The frontmatter's own token count is part of the document, so estimate
    // the count it reports from a draft carrying a same-width placeholder,
    // then fix the number point so the report is exact.
    let draft = frontmatter(engine, generated_at, 0, &refs) + "\n" + &body;
    let mut tokens = estimate_tokens(&draft);
    let mut markdown = draft;
    for _ in 0..8 {
        markdown = frontmatter(engine, generated_at, tokens, &refs) + "\n" + &body;
        let settled = estimate_tokens(&markdown);
        if settled == tokens {
            break;
        }
        tokens = settled;
    }
    Rendered {
        tokens: estimate_tokens(&markdown),
        markdown,
        refs,
    }
}

fn frontmatter(
    engine: &str,
    generated_at: DateTime<Utc>,
    tokens: usize,
    refs: &[ItemId],
) -> String {
    let refs: Vec<&str> = refs.iter().map(ItemId::as_str).collect();
    format!(
        "---\ngenerated_at: {}\nengine: {engine}\ntokens: {tokens}\nrefs: [{}]\n---\n",
        generated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        refs.join(", ")
    )
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_unique(refs: &mut Vec<ItemId>, more: &[ItemId]) {
    for id in more {
        if !refs.contains(id) {
            refs.push(id.clone());
        }
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
