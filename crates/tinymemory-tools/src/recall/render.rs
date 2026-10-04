//! Rendering gathered sections into budgeted markdown.
//!
//! Pure: given the sections and the budget, the output is fixed. A section is
//! either **prose** (an answer) or **lines** (one bullet per hit). Sections
//! come highest priority first, and trimming takes from the end: lines go
//! first, one at a time from the last section that has any, and a section
//! left with none is dropped; then the last prose section is shortened, and
//! dropped once too little of it is left. So earlier sections keep their text
//! longest and every section keeps its place.
//!
//! `context.md` is this renderer with frontmatter: its briefs are prose and
//! its learnings one lines section at the end, so learnings trim first.

use chrono::{DateTime, SecondsFormat, Utc};
use tinymemory_api::ItemId;

/// Characters per estimated token.
const CHARS_PER_TOKEN: usize = 4;

/// A shortened prose section shorter than this is dropped rather than kept
/// as a stub.
const MIN_PROSE_CHARS: usize = 40;

/// Marks a shortened text.
const ELLIPSIS: char = '…';

/// The estimated token count of `text`: four characters per token, rounded
/// up — the estimate every context budget uses.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// One bullet: an item and the text shown for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub(crate) id: ItemId,
    pub(crate) text: String,
}

/// What a section shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Body {
    /// An answer, citing `refs`.
    Prose { text: String, refs: Vec<ItemId> },
    /// One bullet per item, best first.
    Lines(Vec<Line>),
}

/// One gathered section.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Section {
    pub(crate) heading: String,
    pub(crate) body: Body,
}

impl Section {
    fn is_empty(&self) -> bool {
        match &self.body {
            Body::Prose { text, .. } => text.trim().is_empty(),
            Body::Lines(lines) => lines.is_empty(),
        }
    }
}

/// The frontmatter `context.md` carries.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frontmatter<'a> {
    pub(crate) engine: &'a str,
    pub(crate) generated_at: DateTime<Utc>,
}

/// The rendered block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rendered {
    pub(crate) markdown: String,
    pub(crate) tokens: usize,
    pub(crate) refs: Vec<ItemId>,
}

/// Renders `sections` under `title` within `budget_tokens`.
pub(crate) fn render(
    mut sections: Vec<Section>,
    budget_tokens: usize,
    title: &str,
    frontmatter: Option<Frontmatter<'_>>,
) -> Rendered {
    sections.retain(|section| !section.is_empty());
    loop {
        let rendered = compose(&sections, title, frontmatter);
        if rendered.tokens <= budget_tokens {
            return rendered;
        }
        if pop_line(&mut sections) {
            continue;
        }
        let Some(index) = sections
            .iter()
            .rposition(|section| matches!(section.body, Body::Prose { .. }))
        else {
            // Nothing left to trim: an empty document always fits.
            return compose(&sections, title, frontmatter);
        };
        let overflow_chars = (rendered.tokens - budget_tokens) * CHARS_PER_TOKEN;
        if let Body::Prose { text, .. } = &mut sections[index].body {
            let keep = text
                .chars()
                .count()
                .saturating_sub(overflow_chars + ELLIPSIS.len_utf8());
            if keep < MIN_PROSE_CHARS {
                sections.remove(index);
            } else {
                *text = shorten(text, keep);
            }
        }
    }
}

/// Drops the last line of the last section that has lines, and the section
/// with it once it is empty. `false` when no section has lines.
fn pop_line(sections: &mut Vec<Section>) -> bool {
    let Some(index) = sections
        .iter()
        .rposition(|section| matches!(section.body, Body::Lines(_)))
    else {
        return false;
    };
    if let Body::Lines(lines) = &mut sections[index].body {
        lines.pop();
        if lines.is_empty() {
            sections.remove(index);
        }
    }
    true
}

/// The first `keep` characters of `text`, cut back to a word boundary when
/// one is near, with an ellipsis.
pub(crate) fn shorten(text: &str, keep: usize) -> String {
    let cut: String = text.chars().take(keep).collect();
    let trimmed = match cut.rfind(char::is_whitespace) {
        Some(space) if space * 2 > cut.len() => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}{ELLIPSIS}", trimmed.trim_end())
}

/// Composes the full block, frontmatter included, without trimming.
fn compose(sections: &[Section], title: &str, frontmatter: Option<Frontmatter<'_>>) -> Rendered {
    if sections.is_empty() {
        return Rendered {
            markdown: String::new(),
            tokens: 0,
            refs: Vec::new(),
        };
    }
    let mut refs: Vec<ItemId> = Vec::new();
    let mut body = format!("# {title}\n");
    for section in sections {
        match &section.body {
            Body::Prose { text, refs: cited } => {
                body.push_str(&format!("\n## {}\n\n{}\n", section.heading, text.trim()));
                push_unique(&mut refs, cited);
            }
            Body::Lines(lines) => {
                body.push_str(&format!("\n## {}\n\n", section.heading));
                for line in lines {
                    body.push_str(&format!("- {}\n", single_line(&line.text)));
                    push_unique(&mut refs, std::slice::from_ref(&line.id));
                }
            }
        }
    }
    let Some(frontmatter) = frontmatter else {
        return Rendered {
            tokens: estimate_tokens(&body),
            markdown: body,
            refs,
        };
    };
    // The frontmatter's own token count is part of the document, so estimate
    // the count it reports from a draft carrying a same-width placeholder,
    // then fix the number point so the report is exact.
    let draft = header(frontmatter, 0, &refs) + "\n" + &body;
    let mut tokens = estimate_tokens(&draft);
    let mut markdown = draft;
    for _ in 0..8 {
        markdown = header(frontmatter, tokens, &refs) + "\n" + &body;
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

fn header(frontmatter: Frontmatter<'_>, tokens: usize, refs: &[ItemId]) -> String {
    let refs: Vec<&str> = refs.iter().map(ItemId::as_str).collect();
    format!(
        "---\ngenerated_at: {}\nengine: {}\ntokens: {tokens}\nrefs: [{}]\n---\n",
        frontmatter
            .generated_at
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        frontmatter.engine,
        refs.join(", ")
    )
}

/// `text` on one line, whitespace collapsed.
pub(crate) fn single_line(text: &str) -> String {
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
