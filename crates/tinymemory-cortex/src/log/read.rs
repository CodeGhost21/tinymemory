//! Listing a scope's events and building recall packs.

use std::collections::HashSet;

use reqwest::Method;
use serde_json::Value;

use super::{Log, MAX_PAGES, PAGE_SIZE};
use crate::descriptor::Route;
use crate::error::{Error, Result};
use crate::transport::{Attempts, urlencode};

/// Most labels one listing names. Labels share one comma-separated
/// parameter (the hosted backend refuses a repeated `labels=`), so this
/// bounds the URL.
pub(crate) const LABELS_PER_QUERY: usize = 50;

/// One page of a scope listing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Page {
    /// The events, as the engine sent them (duplicates included).
    pub(crate) items: Vec<Value>,
    /// The cursor of the next page; `None` at the end.
    pub(crate) next: Option<String>,
}

impl Log {
    /// One listing page of `scope`, newest first, narrowed to events
    /// carrying any one of `labels` when given.
    pub(crate) async fn page(
        &self,
        scope: &str,
        labels: Option<&[String]>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page> {
        let mut path = format!(
            "{base}?scope={scope}&limit={limit}",
            base = self.client.wire().path(Route::Events),
            scope = urlencode(scope),
        );
        if let Some(labels) = labels.filter(|labels| !labels.is_empty()) {
            path.push_str(&format!("&labels={}", urlencode(&labels.join(","))));
        }
        if let Some(cursor) = cursor {
            path.push_str(&format!("&cursor={}", urlencode(cursor)));
        }
        let page = self
            .client
            .json(Method::GET, &path, None, Attempts::RetryTransient)
            .await?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next = match (
            page.get("has_more").and_then(Value::as_bool),
            page.get("next_cursor").and_then(Value::as_str),
        ) {
            (Some(true), Some(next)) => Some(next.to_string()),
            _ => None,
        };
        if next.is_some() && next.as_deref() == cursor {
            return Err(Error::Engine(format!(
                "listing scope `{scope}` returned the cursor it was given; refusing to walk a \
                 listing that does not advance"
            )));
        }
        Ok(Page { items, next })
    }

    /// Every distinct event of `scope` carrying any one of `labels` (all of
    /// them when `labels` is `None`), newest first, following the cursor to
    /// the end and dropping the engine's duplicate copies by event id.
    ///
    /// # Errors
    ///
    /// Backend failures, and [`Error::Engine`] past [`MAX_PAGES`] pages.
    pub(crate) async fn walk(&self, scope: &str, labels: Option<&[String]>) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self
                .page(scope, labels, cursor.as_deref(), PAGE_SIZE)
                .await?;
            for item in page.items {
                match item.get("id").and_then(Value::as_str) {
                    Some(id) if !seen.insert(id.to_string()) => {}
                    _ => all.push(item),
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(all),
            }
        }
        Err(Error::Engine(format!(
            "listing scope `{scope}` exceeded {MAX_PAGES} pages; refusing to answer from a \
             truncated log"
        )))
    }

    /// [`Self::walk`] for many labels, in batches of [`LABELS_PER_QUERY`],
    /// deduplicated across batches.
    pub(crate) async fn walk_labels(&self, scope: &str, labels: &[String]) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        for batch in labels.chunks(LABELS_PER_QUERY) {
            for event in self.walk(scope, Some(batch)).await? {
                let fresh = event
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|id| seen.insert(id.to_string()));
                if fresh {
                    all.push(event);
                }
            }
        }
        Ok(all)
    }

    /// Builds a recall pack. A read: retried on transient failures.
    pub(crate) async fn recall(&self, body: &Value) -> Result<Value> {
        self.client
            .json(
                Method::POST,
                self.client.wire().path(Route::Recall),
                Some(body),
                Attempts::RetryTransient,
            )
            .await
    }

    /// Asks the answer route once, with a pack already built.
    pub(crate) async fn answer(&self, body: &Value) -> Result<Value> {
        self.client
            .json(
                Method::POST,
                self.client.wire().path(Route::Answer),
                Some(body),
                Attempts::Once,
            )
            .await
    }
}
