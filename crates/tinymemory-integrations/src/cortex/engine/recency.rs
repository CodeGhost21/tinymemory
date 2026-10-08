//! Which of a fetch's scopes it reads when it may read only a few
//! ([`tinymemory_api::FetchRequest::max_scopes`]).
//!
//! Each scope is a separate recall pack (a query embedding and a ranking on
//! the server), so a brain split per connector and repository would cost a
//! pack per scope on every turn. A capped fetch keeps:
//!
//! 1. the scopes the query **names**: a query word (three letters or more)
//!    equal to one of the scope's segment ids or to a part of one split at
//!    `-` and `_` (`notion`, `github`, `api` for `project:acme--api`);
//! 2. then the most **recently written**: the newest `observed_at` among the
//!    scope's latest events, or the time this engine last wrote to it;
//! 3. then the scope order the fetch would read in.
//!
//! What is kept is read in the fetch's own scope order. A scope's recency is
//! learned once from one short listing (newest first, no embedding), kept
//! for [`TTL`], and set to now by every write this engine makes, so a warm
//! engine adds no request to a turn. A listing that fails leaves the scope
//! with no recency: it is kept only when the others leave room.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use futures::{StreamExt, stream};
use serde_json::Value;
use tinymemory_api::chrono::{DateTime, Utc};

use super::CortexEngine;
use super::scopes::KindScope;

/// How long a learned recency is trusted before it is listed again.
const TTL: Duration = Duration::from_secs(10 * 60);

/// Events one recency listing asks for: the engine sends each twice, so
/// about ten distinct events, enough that one late-dated sync does not hide
/// the scope's newest.
const LISTED_EVENTS: usize = 20;

/// Recency listings sent at once.
const LISTINGS_AT_ONCE: usize = 8;

/// The shortest query word that can name a scope.
const MIN_WORD: usize = 3;

/// The last known write time of each scope (shared by clones).
#[derive(Debug, Default)]
pub(super) struct Recency {
    seen: Mutex<HashMap<String, (Option<DateTime<Utc>>, Instant)>>,
}

impl Recency {
    /// `scope`'s recency when it is known and fresh: `Some(None)` for a
    /// scope known to hold no dated event.
    fn get(&self, scope: &str) -> Option<Option<DateTime<Utc>>> {
        let seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.get(scope)
            .filter(|(_, learned)| learned.elapsed() < TTL)
            .map(|(at, _)| *at)
    }

    fn set(&self, scope: &str, at: Option<DateTime<Utc>>) {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(scope.to_string(), (at, Instant::now()));
    }

    /// Records that this engine has just written to `scope`.
    pub(super) fn touch(&self, scope: &str) {
        self.set(scope, Some(DateTime::<Utc>::from(SystemTime::now())));
    }
}

impl CortexEngine {
    /// At most `max` of `scopes` for `query` (see the module docs), in their
    /// given order.
    pub(super) async fn pick_scopes(
        &self,
        scopes: Vec<KindScope>,
        query: &str,
        max: usize,
    ) -> Vec<KindScope> {
        let unknown: Vec<String> = scopes
            .iter()
            .filter(|scope| self.recency.get(&scope.path).is_none())
            .map(|scope| scope.path.clone())
            .collect();
        stream::iter(unknown)
            .for_each_concurrent(LISTINGS_AT_ONCE, |path| async move {
                let at = match self.log.page(&path, None, None, LISTED_EVENTS).await {
                    Ok(page) => newest(&page.items),
                    Err(error) => {
                        log::debug!("[cortex] recency of {path} unknown: {error}");
                        None
                    }
                };
                self.recency.set(&path, at);
            })
            .await;
        let total = scopes.len();
        let dated: Vec<(KindScope, Option<DateTime<Utc>>)> = scopes
            .into_iter()
            .map(|scope| {
                let at = self.recency.get(&scope.path).flatten();
                (scope, at)
            })
            .collect();
        let kept = choose(dated, &words(query), max);
        log::debug!("[cortex] fetch reads {} of {total} scopes", kept.len());
        kept
    }
}

/// The lowercase words of `query` long enough to name a scope.
fn words(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= MIN_WORD)
        .map(str::to_lowercase)
        .collect()
}

/// Whether one of `words` names `scope`: equals a segment id, or a part of
/// one split at `-` and `_`.
fn named(words: &[String], scope: &KindScope) -> bool {
    scope.namespace.segments().iter().any(|segment| {
        let id = segment.id().to_lowercase();
        words.iter().any(|word| {
            *word == id
                || id
                    .split(['-', '_'])
                    .any(|part| part.chars().count() >= MIN_WORD && part == word.as_str())
        })
    })
}

/// The newest `context.observed_at` among `events`.
fn newest(events: &[Value]) -> Option<DateTime<Utc>> {
    events
        .iter()
        .filter_map(|event| event.pointer("/context/observed_at")?.as_str())
        .filter_map(|at| DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&Utc))
        .max()
}

/// At most `max` of `dated`: named first, then newest, then in order; the
/// kept ones in their given order.
fn choose(
    dated: Vec<(KindScope, Option<DateTime<Utc>>)>,
    words: &[String],
    max: usize,
) -> Vec<KindScope> {
    let mut ranked: Vec<(usize, bool, Option<DateTime<Utc>>, KindScope)> = dated
        .into_iter()
        .enumerate()
        .map(|(index, (scope, at))| (index, named(words, &scope), at, scope))
        .collect();
    // Named before unnamed, newer before older (an undated scope last), then
    // the given order.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    ranked.truncate(max);
    ranked.sort_by_key(|(index, ..)| *index);
    ranked.into_iter().map(|(.., scope)| scope).collect()
}

#[cfg(test)]
#[path = "recency_tests.rs"]
mod tests;
