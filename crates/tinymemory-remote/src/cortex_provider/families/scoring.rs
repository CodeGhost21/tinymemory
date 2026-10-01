//! Scoring: the query-side helpers a host grounds retrieval with.
//!
//! Entity extraction runs here, on the device, as the embedded engine's
//! fallback extractor does when no NLP service is running: emails, URLs,
//! `@handles`, `name#1234` discriminators and `#hashtags`, each as a canonical
//! `<kind>:<value>`, and a hashtag as a topic too. It never fails.
//!
//! Embedding does not: the hosted engine embeds what it stores, on its own
//! servers, and the memory API offers no route to embed text on request. So
//! [`MemoryScoring::embed_text`] answers `Unsupported`, and a host that would
//! embed a segment's recap skips it — the engine has the recap's text anyway.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::MemoryScoring;

use crate::cortex_provider::CortexProvider;

/// Characters that end a URL in running text rather than belong to it.
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?'];

fn is_email(token: &str) -> bool {
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
        && !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

fn is_handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The handle after a leading `@`, trimmed to end on a letter, digit or `_`.
fn handle(token: &str) -> Option<&str> {
    let body = token.strip_prefix('@')?;
    let end = body
        .char_indices()
        .take_while(|(_, c)| is_handle_char(*c) || *c == '.' || *c == '-')
        .map(|(at, c)| at + c.len_utf8())
        .last()?;
    let body = body[..end].trim_end_matches(['.', '-']);
    body.starts_with(is_handle_char).then_some(body)
}

/// A `name#1234` discriminator handle.
fn discriminator(token: &str) -> Option<&str> {
    let (name, digits) = token.split_once('#')?;
    let digits = digits.trim_end_matches(TRAILING);
    let fits = (2..=32).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        && digits.len() == 4
        && digits.chars().all(|c| c.is_ascii_digit());
    fits.then(|| &token[..name.len() + 5])
}

/// The tag after a leading `#`: a letter, then at least one more of letters,
/// digits, `_` and `-`.
fn hashtag(token: &str) -> Option<&str> {
    let body = token.strip_prefix('#')?;
    let end = body
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .map(|(at, c)| at + c.len_utf8())
        .last()?;
    let tag = &body[..end];
    (tag.starts_with(|c: char| c.is_ascii_alphabetic()) && tag.len() >= 2).then_some(tag)
}

/// The canonical `<kind>:<value>` ids of the entities in `text`, in order of
/// kind and then of appearance, without repeats.
pub(super) fn extract_entities(text: &str) -> Vec<String> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || "<>[]\"'".contains(c))
        .map(|token| token.trim_start_matches('('))
        .map(|token| token.trim_end_matches(')'))
        .filter(|token| !token.is_empty())
        .collect();
    let mut found: Vec<String> = Vec::new();
    let mut push = |id: String| {
        if !found.contains(&id) {
            found.push(id);
        }
    };
    for token in &tokens {
        let token = token.trim_end_matches(TRAILING);
        if is_email(token) {
            push(format!("email:{}", token.to_lowercase()));
        }
    }
    for token in &tokens {
        if token.starts_with("http://") || token.starts_with("https://") {
            let url = token.trim_end_matches(TRAILING);
            if url.contains("://") && !url.ends_with("://") {
                push(format!("url:{url}"));
            }
        }
    }
    for token in &tokens {
        if let Some(handle) = handle(token) {
            push(format!("handle:{}", handle.to_lowercase()));
        }
    }
    for token in &tokens {
        if let Some(handle) = discriminator(token) {
            push(format!("handle:{}", handle.to_lowercase()));
        }
    }
    let mut topics = Vec::new();
    for token in &tokens {
        if let Some(tag) = hashtag(token) {
            let tag = tag.to_lowercase();
            push(format!("hashtag:{tag}"));
            topics.push(format!("topic:{tag}"));
        }
    }
    for topic in topics {
        push(topic);
    }
    found
}

#[async_trait]
impl MemoryScoring for CortexProvider {
    async fn extract_entities(&self, query: &str) -> Result<Vec<String>, MemoryError> {
        Ok(extract_entities(query))
    }

    async fn embed_text(&self, _text: &str) -> Result<Vec<f32>, MemoryError> {
        Err(MemoryError::unsupported_raw("scoring.embed_text"))
    }

    /// The hosted engine embeds in the cloud.
    async fn embedder_slug(&self) -> Result<String, MemoryError> {
        Ok("cloud".to_string())
    }
}

#[cfg(test)]
#[path = "scoring_test.rs"]
mod test;
