//! HTML to markdown.
//!
//! A small, dependency-free structural converter, not a browser. It walks the
//! tag stream once and keeps the structure that survives being stored as
//! memory — headings, paragraphs, lists, links, code, block quotes, emphasis —
//! and discards the rest.
//!
//! ## Why not a real HTML parser
//!
//! Because the output is prose for a language model to read, and the failure
//! modes of a tag-stream walk are all cosmetic: a malformed nesting produces
//! slightly wrong emphasis, never wrong text. Pulling in a full DOM parser
//! would cost this crate its "no heavy dependencies" position for output
//! nobody renders. If a host needs fidelity beyond this, it supplies its own
//! [`crate::convert::DocumentConverter`].
//!
//! Script and style bodies are removed before anything else, so their contents
//! can never reach the output as text. `<title>` goes with them: it is document
//! metadata, [`extract_title`] reads it from the original source, and leaving it
//! in would open every converted page with its own title as a stray line of
//! prose.

mod entity;

use entity::decode_entities;

/// Convert an HTML document to markdown.
///
/// Never fails: HTML has no error state this converter can be pushed into, and
/// malformed input degrades to slightly worse markdown rather than to an error
/// a caller would have to handle.
pub fn to_markdown(html: &str) -> String {
    let cleaned = strip_raw_text_elements(html);
    let mut out = Renderer::default();
    let mut rest = cleaned.as_str();

    while let Some(open) = rest.find('<') {
        out.text(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            // An unterminated '<' is literal text, not a tag.
            out.text(&rest[open..]);
            rest = "";
            break;
        };
        out.tag(&after[..close]);
        rest = &after[close + 1..];
    }
    out.text(rest);
    out.finish()
}

/// Extract the contents of `<title>`, if the document has one.
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let content_start = lower[start..].find('>')? + start + 1;
    let end = lower[content_start..].find("</title>")? + content_start;
    let title = decode_entities(html.get(content_start..end)?)
        .trim()
        .to_string();
    (!title.is_empty()).then_some(title)
}

/// Remove `<script>`, `<style>`, `<template>`, `<svg>` and comment bodies.
///
/// Done as a pre-pass rather than inside the walk because their contents are
/// *not* markup — a `>` inside a JavaScript string would otherwise terminate a
/// tag that never started, and the walk would emit code as prose.
fn strip_raw_text_elements(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let lower = |s: &str| s.to_ascii_lowercase();

    'outer: while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let tail = &rest[open..];

        if let Some(after) = tail.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }

        for name in ["script", "style", "template", "svg", "noscript", "title"] {
            let head = lower(&tail[..tail.len().min(name.len() + 1)]);
            if head == format!("<{name}") {
                // `<scripture>` and `<script-x>` share this prefix but are not
                // the element being matched; only `>`, `/`, or whitespace
                // after the name is a real tag-name boundary.
                let after_name = &tail[(name.len() + 1).min(tail.len())..];
                let is_boundary = after_name
                    .chars()
                    .next()
                    .is_some_and(|c| c == '>' || c == '/' || c.is_ascii_whitespace());
                if !is_boundary {
                    continue;
                }
                let closing = format!("</{name}");
                let lowered = lower(tail);
                match lowered[1..].find(&closing) {
                    Some(at) => {
                        let from = 1 + at;
                        rest = match tail[from..].find('>') {
                            Some(end) => &tail[from + end + 1..],
                            None => "",
                        };
                    }
                    // An unclosed script swallows the rest of the document,
                    // which is what a browser does too.
                    None => rest = "",
                }
                continue 'outer;
            }
        }

        // Not a stripped element: copy the tag through verbatim.
        match tail.find('>') {
            Some(end) => {
                out.push_str(&tail[..=end]);
                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(tail);
                break;
            }
        }
    }
    out
}

/// Accumulates markdown while the tag stream is walked.
#[derive(Default)]
struct Renderer {
    out: String,
    /// Nesting depth of open list elements; `Some(index)` for an ordered list.
    lists: Vec<Option<u32>>,
    /// Set while inside `<pre>`, where whitespace is significant.
    preformatted: bool,
    /// The href of the anchor currently being written, if any.
    link: Option<String>,
    /// Text collected since the anchor opened.
    link_text: String,
    /// True once a block-level break is pending, so consecutive block tags do
    /// not produce a run of blank lines.
    pending_break: bool,
}

impl Renderer {
    fn text(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        let decoded = decode_entities(raw);
        if self.preformatted {
            self.push_raw(&decoded);
            return;
        }
        let collapsed = collapse_whitespace(&decoded);
        if collapsed.trim().is_empty() {
            // Whitespace between block tags is layout, not content — but
            // whitespace between two inline runs is a word boundary.
            if !collapsed.is_empty() && !self.pending_break && self.ends_with_word() {
                self.push_raw(" ");
            }
            return;
        }
        self.push_raw(&collapsed);
    }

    fn push_raw(&mut self, text: &str) {
        if self.link.is_some() {
            self.link_text.push_str(text);
            return;
        }
        self.flush_break();
        self.out.push_str(text);
    }

    fn ends_with_word(&self) -> bool {
        let target = if self.link.is_some() {
            &self.link_text
        } else {
            &self.out
        };
        target.chars().last().is_some_and(|c| !c.is_whitespace())
    }

    /// Emit the pending blank line, if one is owed.
    fn flush_break(&mut self) {
        if !self.pending_break {
            return;
        }
        self.pending_break = false;
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with("\n\n") {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push('\n');
        }
    }

    fn block_break(&mut self) {
        self.pending_break = true;
    }

    fn tag(&mut self, inner: &str) {
        let inner = inner.trim();
        let closing = inner.starts_with('/');
        let body = inner.trim_start_matches('/');
        let name = body
            .split([' ', '\t', '\n', '/'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block_break();
                if !closing {
                    let level = name[1..].parse::<usize>().unwrap_or(1);
                    self.flush_break();
                    self.out.push_str(&"#".repeat(level));
                    self.out.push(' ');
                }
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "table" | "tr"
            | "blockquote" | "hr" => {
                self.block_break();
                if name == "hr" {
                    self.flush_break();
                    self.out.push_str("---");
                    self.block_break();
                } else if name == "blockquote" && !closing {
                    self.flush_break();
                    self.out.push_str("> ");
                }
            }
            "br" => {
                if !self.preformatted {
                    self.flush_break();
                    self.out.push_str("  \n");
                }
            }
            "ul" | "ol" => {
                self.block_break();
                if closing {
                    self.lists.pop();
                } else {
                    self.lists.push((name == "ol").then_some(1));
                }
            }
            "li" => {
                if closing {
                    return;
                }
                self.flush_break();
                if !self.out.is_empty() && !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"  ".repeat(depth));
                match self.lists.last_mut() {
                    Some(Some(index)) => {
                        self.out.push_str(&format!("{index}. "));
                        *index += 1;
                    }
                    _ => self.out.push_str("- "),
                }
            }
            "pre" => {
                self.block_break();
                self.flush_break();
                self.preformatted = !closing;
                self.out.push_str("```");
                self.out.push('\n');
                if closing {
                    self.block_break();
                }
            }
            "code" | "tt" => {
                if !self.preformatted {
                    self.push_raw("`");
                }
            }
            "strong" | "b" => self.push_raw("**"),
            "em" | "i" => self.push_raw("*"),
            "a" => {
                if closing {
                    self.close_link();
                } else {
                    self.open_link(body);
                }
            }
            "td" | "th" if !closing && self.ends_with_word() => self.push_raw(" | "),
            _ => {}
        }
    }

    fn open_link(&mut self, body: &str) {
        // A nested anchor is malformed; treat the inner one as text so the
        // outer link is not lost.
        if self.link.is_some() {
            return;
        }
        self.link = Some(attribute(body, "href").unwrap_or_default());
        self.link_text.clear();
    }

    fn close_link(&mut self) {
        let Some(href) = self.link.take() else {
            return;
        };
        let text = std::mem::take(&mut self.link_text);
        let text = text.trim();
        if text.is_empty() && href.is_empty() {
            return;
        }
        self.flush_break();
        if href.is_empty() {
            self.out.push_str(text);
        } else if text.is_empty() {
            self.out.push_str(&format!("<{href}>"));
        } else {
            self.out.push_str(&format!("[{text}]({href})"));
        }
    }

    fn finish(mut self) -> String {
        // An unclosed anchor still has text worth keeping.
        if self.link.is_some() {
            self.close_link();
        }
        let mut text = std::mem::take(&mut self.out);
        while text.contains("\n\n\n") {
            text = text.replace("\n\n\n", "\n\n");
        }
        text.trim().to_string()
    }
}

/// Read one attribute out of a tag body.
///
/// Handles single quotes, double quotes and unquoted values, because all three
/// appear in the wild and a converter that only read double-quoted `href`s
/// would silently drop links.
fn attribute(body: &str, name: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let at = lower[from..].find(name)? + from;
        let before_ok = at == 0
            || lower[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &body[at + name.len()..];
        let trimmed = rest.trim_start();
        if before_ok {
            if let Some(value) = trimmed.strip_prefix('=') {
                let value = value.trim_start();
                let decoded = match value.chars().next() {
                    Some('"') => value[1..].split('"').next().map(str::to_string),
                    Some('\'') => value[1..].split('\'').next().map(str::to_string),
                    _ => value
                        .split([' ', '\t', '\n', '>'])
                        .next()
                        .map(str::to_string),
                };
                return decoded.map(|v| decode_entities(&v));
            }
        }
        from = at + name.len();
    }
}

/// Collapse every run of whitespace to a single space.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
