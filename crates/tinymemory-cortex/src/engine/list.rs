//! List: a cursor over the admitted kind scopes' event listings.
//!
//! The scopes are read in [`ItemKind::ALL`] order, each newest first. Every
//! raw event is decoded and kept when it is one of this crate's envelopes of
//! the scope's kind and the full [`tinymemory_api::MetaFilter`] matches. When
//! the filter has a labelled field, the listing is narrowed server-side by
//! that one label first (see `envelope::labels`); the client-side check runs
//! regardless, and the cursor stays the engine's.
//!
//! **Each item once.** A document or learning is one event. A conversation
//! is emitted only on the page holding its turn-0 event, and its text is
//! assembled from all its turns by one label lookup per page. Writes are
//! ordered, so a conversation whose store failed part-way still has its
//! turn 0 and lists with the turns it holds.
//!
//! **Duplicates.** The engine emits each event twice in a row; a copy equal
//! to the previous raw event is skipped, across page boundaries too (the
//! cursor remembers the last id). A page that ends mid-way is resumed by
//! re-reading the same engine page and skipping the consumed events.

use std::collections::HashSet;

use serde_json::Value;
use tinymemory_api::{Hit, ItemKind, ListPage, ListRequest};

use super::CortexEngine;
use super::cursor::{self, ListCursor};
use super::items::{admitted, hit, keeps};
use crate::envelope::{Envelope, decode_event, labels, rebuild, scope_of};
use crate::error::{Error, Result};
use crate::log::{MAX_PAGES, PAGE_SIZE};

/// The cursor tag of a listing.
const TAG: char = 'l';

/// A hit, or a conversation whose turns are assembled before the page
/// returns.
enum Pending {
    Ready(Hit),
    Conversation(String),
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn list_page(&self, req: ListRequest) -> Result<ListPage> {
        req.validate()?;
        let kinds = admitted(&req.filter);
        let Some(first) = kinds.first() else {
            return Ok(ListPage::default());
        };
        let mut at = match &req.cursor {
            Some(raw) => cursor::decode::<ListCursor>(TAG, raw)?,
            None => ListCursor::at(*first),
        };
        let narrowing = labels::narrowing(&req.filter);
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        let mut pages = 0;
        let mut next = None;
        'scopes: for (index, kind) in ItemKind::ALL.into_iter().enumerate() {
            if index < at.scope || !kinds.contains(&kind) {
                continue;
            }
            if index > at.scope {
                at = ListCursor::at(kind);
            }
            loop {
                pages += 1;
                if pages > MAX_PAGES {
                    return Err(Error::Engine(format!(
                        "listing read {MAX_PAGES} pages without filling a page of results; \
                         refusing to walk further"
                    )));
                }
                let page = self
                    .log
                    .page(
                        scope_of(kind),
                        narrowing.as_deref(),
                        at.engine.as_deref(),
                        PAGE_SIZE,
                    )
                    .await?;
                let len = page.items.len();
                for (position, event) in page.items.iter().enumerate().skip(at.offset) {
                    at.offset = position + 1;
                    let id = event.get("id").and_then(Value::as_str);
                    if id.is_some() && id == at.last.as_deref() {
                        continue;
                    }
                    at.last = id.map(str::to_owned);
                    if let Some(found) = self.admit(kind, &req, event, &mut seen) {
                        pending.push(found);
                        if pending.len() == req.limit {
                            let exhausted = at.offset == len
                                && page.next.is_none()
                                && kinds.last() == Some(&kind);
                            if !exhausted {
                                if at.offset == len
                                    && let Some(engine) = &page.next
                                {
                                    at.engine = Some(engine.clone());
                                    at.offset = 0;
                                }
                                next = Some(cursor::encode(TAG, &at)?);
                            }
                            break 'scopes;
                        }
                    }
                }
                match page.next {
                    Some(engine) => {
                        at.engine = Some(engine);
                        at.offset = 0;
                    }
                    None => break,
                }
            }
        }
        Ok(ListPage {
            items: self.resolve(pending).await?,
            next_cursor: next,
        })
    }

    /// Whether one raw event starts an item this listing returns.
    fn admit(
        &self,
        kind: ItemKind,
        req: &ListRequest,
        event: &Value,
        seen: &mut HashSet<String>,
    ) -> Option<Pending> {
        let envelope = decode_event(event)?.envelope;
        if !keeps(&req.filter, kind, &envelope) {
            return None;
        }
        let starts = envelope.turn.as_ref().is_none_or(|turn| turn.index == 0);
        if !starts || !seen.insert(envelope.id.clone()) {
            return None;
        }
        if kind == ItemKind::Conversation {
            return Some(Pending::Conversation(envelope.id));
        }
        let id = envelope.id.clone();
        let item = rebuild(std::slice::from_ref::<Envelope>(&envelope))?;
        Some(Pending::Ready(hit(&id, &item, 0.0)))
    }

    /// Assembles the page's conversations (one lookup for all of them) and
    /// returns the hits in listing order.
    async fn resolve(&self, pending: Vec<Pending>) -> Result<Vec<Hit>> {
        let ids: Vec<String> = pending
            .iter()
            .filter_map(|p| match p {
                Pending::Conversation(id) => Some(id.clone()),
                Pending::Ready(_) => None,
            })
            .collect();
        let conversations = self.conversations(&ids).await?;
        Ok(pending
            .into_iter()
            .filter_map(|p| match p {
                Pending::Ready(hit) => Some(hit),
                Pending::Conversation(id) => conversations.get(&id).map(|item| hit(&id, item, 0.0)),
            })
            .collect())
    }
}
