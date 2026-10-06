//! Splitting a document's text into the events it is written as.
//!
//! CortexDB refuses an experience whose flattened text is over 1 MiB
//! (`422 INVALID_ENVELOPE`, never truncated). A document's event text is its
//! whole [`super::Envelope`], so a long document is written as several
//! events, each carrying one contiguous piece of the body; read back in order
//! they concatenate to the body exactly.
//!
//! **Units.** The text is first cut where its structure says to: at every
//! page break ([`PAGE_BREAK`], which the PDF converter puts between pages),
//! and within a page before every markdown heading line. A unit never spans
//! two pages.
//!
//! **Packing.** Units are packed greedily, in order, into pieces of at most
//! [`DOCUMENT_CHUNK_TARGET_BYTES`]. A unit larger than that is cut finer, at
//! blank lines, then line ends, then characters, and those parts packed the
//! same way. Every size is the piece's cost inside the encoded envelope (its
//! JSON-escaped length), so a piece and the envelope around it stay under
//! [`MAX_EVENT_TEXT_BYTES`].
//!
//! A document that fits in one piece is one event, exactly as before
//! chunking existed.

/// The page break the PDF converter writes between pages: a form feed, the
/// same character as `documents::PAGE_BREAK` (this module does not depend on
/// the `documents` feature).
pub(crate) const PAGE_BREAK: char = '\u{c}';

/// The most one event's text (its encoded envelope) may take: CortexDB's
/// 1 MiB limit with a quarter left as a safety margin, because the server
/// counts its own flattening of the text, which this crate cannot see.
pub(crate) const MAX_EVENT_TEXT_BYTES: usize = 768 * 1024;

/// How much of a document's body one event carries at most, as encoded
/// bytes.
///
/// The one knob for granularity. At this value a document under it stays
/// one event and a longer one is packed into as few events as fit. At `0`
/// every page and every section is its own event, and a part is cut finer
/// only when it alone would exceed [`MAX_EVENT_TEXT_BYTES`].
pub(crate) const DOCUMENT_CHUNK_TARGET_BYTES: usize = 256 * 1024;

/// The longest section title an event carries, in characters.
pub(crate) const MAX_SECTION_CHARS: usize = 120;

/// One piece of a document: a contiguous slice of its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece<'a> {
    /// The text, exactly as it appears in the document.
    pub(crate) text: &'a str,
    /// The pages it covers, first and last, counted from 1, when the
    /// document marks its pages.
    pub(crate) pages: Option<(u32, u32)>,
    /// The title of the section it starts in, when one was seen.
    pub(crate) section: Option<String>,
}

/// A unit before packing: a byte range, its page, and its section.
struct Unit {
    start: usize,
    end: usize,
    page: u32,
    section: Option<String>,
}

/// The pieces of `text`, each at most `target` (or, at `0`, one per unit)
/// and never over `limit`, both as encoded bytes on top of `overhead` (the
/// envelope around the piece). One piece for a text that fits.
pub(crate) fn split(text: &str, overhead: usize, target: usize, limit: usize) -> Vec<Piece<'_>> {
    let room = limit.saturating_sub(overhead).max(1);
    let pack = if target == 0 {
        room
    } else {
        target.saturating_sub(overhead).clamp(1, room)
    };
    let paged = text.contains(PAGE_BREAK);
    if target != 0 && escaped_len(text) <= pack {
        return vec![Piece {
            text,
            pages: paged.then(|| (1, page_count(text))),
            section: None,
        }];
    }
    let mut pieces: Vec<Piece<'_>> = Vec::new();
    let mut open: Option<Open> = None;
    for unit in units(text) {
        for (start, end) in parts(text, unit.start, unit.end, pack) {
            let size = escaped_len(&text[start..end]);
            match open.as_mut() {
                Some(current) if target != 0 && current.size + size <= pack => {
                    current.end = end;
                    current.last_page = unit.page;
                    current.size += size;
                }
                _ => {
                    if let Some(done) = open.take() {
                        pieces.push(done.piece(text, paged));
                    }
                    open = Some(Open {
                        start,
                        end,
                        first_page: unit.page,
                        last_page: unit.page,
                        section: unit.section.clone(),
                        size,
                    });
                }
            }
        }
    }
    pieces.extend(open.map(|done| done.piece(text, paged)));
    pieces
}

/// The piece being packed.
struct Open {
    start: usize,
    end: usize,
    first_page: u32,
    last_page: u32,
    section: Option<String>,
    size: usize,
}

impl Open {
    fn piece(self, text: &str, paged: bool) -> Piece<'_> {
        Piece {
            text: &text[self.start..self.end],
            pages: paged.then_some((self.first_page, self.last_page)),
            section: self.section,
        }
    }
}

/// The units of `text`: one per page, cut again before every heading line.
/// A stretch holding only whitespace and page breaks is never a unit of its
/// own; it joins the unit that follows.
fn units(text: &str) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut page = 1;
    let mut section: Option<String> = None;
    let mut start = 0;
    let mut start_page = 1;
    let mut line_start = 0;
    for (at, ch) in text.char_indices() {
        let breaks_page = ch == PAGE_BREAK;
        let title = if at == line_start {
            heading(&text[at..])
        } else {
            None
        };
        if (breaks_page || title.is_some()) && !blank(&text[start..at]) {
            units.push(Unit {
                start,
                end: at,
                page: start_page,
                section: section.clone(),
            });
            start = at;
            start_page = page;
        }
        if breaks_page {
            page += 1;
            if blank(&text[start..at]) {
                start_page = page;
            }
        }
        if let Some(title) = title {
            section = Some(title);
        }
        if ch == '\n' || breaks_page {
            line_start = at + ch.len_utf8();
        }
    }
    if start < text.len() {
        units.push(Unit {
            start,
            end: text.len(),
            page: start_page,
            section,
        });
    }
    units
}

/// Whether `text` holds nothing but whitespace and page breaks.
fn blank(text: &str) -> bool {
    text.chars().all(|c| c == PAGE_BREAK || c.is_whitespace())
}

/// The title of a markdown heading line starting at the head of `rest`
/// (`#` to `######`, a space, then text), trimmed and shortened.
fn heading(rest: &str) -> Option<String> {
    let line = rest.split('\n').next().unwrap_or_default();
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &trimmed[hashes..];
    if !after.starts_with(' ') {
        return None;
    }
    let title = after.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then(|| title.chars().take(MAX_SECTION_CHARS).collect())
}

/// `text[start..end]` cut into ranges of at most `cap` encoded bytes: whole
/// when it fits, else at blank lines, then line ends, then characters.
fn parts(text: &str, start: usize, end: usize, cap: usize) -> Vec<(usize, usize)> {
    if escaped_len(&text[start..end]) <= cap {
        return vec![(start, end)];
    }
    for separator in ["\n\n", "\n"] {
        let cuts = cut_after(text, start, end, separator);
        if cuts.len() > 1 {
            return cuts
                .into_iter()
                .flat_map(|(s, e)| parts(text, s, e, cap))
                .collect::<Vec<_>>()
                .into_iter()
                .fold(Vec::new(), |mut packed: Vec<(usize, usize)>, (s, e)| {
                    match packed.last_mut() {
                        Some(last) if escaped_len(&text[last.0..e]) <= cap => last.1 = e,
                        _ => packed.push((s, e)),
                    }
                    packed
                });
        }
    }
    let mut ranges = Vec::new();
    let mut from = start;
    let mut used = 0;
    for (at, ch) in text[start..end].char_indices() {
        let size = escaped_char_len(ch);
        if used + size > cap && start + at > from {
            ranges.push((from, start + at));
            from = start + at;
            used = 0;
        }
        used += size;
    }
    ranges.push((from, end));
    ranges
}

/// `text[start..end]` cut just after every `separator`.
fn cut_after(text: &str, start: usize, end: usize, separator: &str) -> Vec<(usize, usize)> {
    let mut cuts = Vec::new();
    let mut from = start;
    for (at, _) in text[start..end].match_indices(separator) {
        let to = start + at + separator.len();
        if to < end {
            cuts.push((from, to));
            from = to;
        }
    }
    cuts.push((from, end));
    cuts
}

/// How many pages `text` marks: one more than its page breaks.
fn page_count(text: &str) -> u32 {
    let breaks = text.matches(PAGE_BREAK).count();
    u32::try_from(breaks).map_or(u32::MAX, |breaks| breaks.saturating_add(1))
}

/// The length of `text` as a JSON string body, as `serde_json` escapes it.
pub(crate) fn escaped_len(text: &str) -> usize {
    text.chars().map(escaped_char_len).sum()
}

fn escaped_char_len(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

#[cfg(test)]
#[path = "chunks_tests.rs"]
mod tests;
