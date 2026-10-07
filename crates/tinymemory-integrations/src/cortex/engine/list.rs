//! List: a cursor over the event listings of the scopes the filter reads.
//!
//! The scopes (each admitted kind at each namespace node in reach, see
//! `scopes`) are read in [`ItemKind::ALL`] order and then by namespace, each
//! newest first. Every raw event is decoded and kept when it is one of this
//! crate's envelopes of the scope's kind and the full
//! [`tinymemory_api::MetaFilter`] matches. When
//! the filter has a labelled field, the listing is narrowed server-side by
//! that one label first (see `envelope::labels`); the client-side check runs
//! regardless, and the cursor stays the engine's.
//!
//! **Each item once.** A learning, or a document written whole, is one
//! event. A conversation, or a chunked document, is emitted only on the page
//! holding its first event (turn 0, piece 0), and its text is assembled from
//! all its events by one label lookup per page and kind. Writes are
//! ordered, so a conversation whose store failed part-way still has its
//! turn 0 and lists with the turns it holds.
//!
//! **Duplicates.** The engine emits each event twice in a row; a copy equal
//! to the previous raw event is skipped, across page boundaries too (the
//! cursor remembers the last id). A page that ends mid-way is resumed by
//! re-reading the same engine page and skipping the consumed events.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tinymemory_api::{
    ExportPage, Exported, ItemId, ItemKind, ListPage, ListRequest, Namespace, StoreItem,
};

use super::CortexEngine;
use super::cursor::{self, ListCursor};
use super::items::{hit, keeps};
use super::scopes::KindScope;
use crate::cortex::envelope::{Envelope, decode_event, labels, parse_scope, rebuild};
use crate::cortex::error::{Error, Result};
use crate::cortex::log::{MAX_PAGES, PAGE_SIZE};

/// The cursor tag of a listing.
const TAG: char = 'l';

/// An item rebuilt from its one event, or one whose events (a
/// conversation's turns, a chunked document's pieces) are assembled before
/// the page returns.
enum Pending {
    Ready(String, Box<StoreItem>),
    Assembled(ItemKind, String, Namespace),
}

/// A page of items in listing order, each with its id; `None` for an item
/// whose events could not be assembled whole.
type ItemsPage = (Vec<(String, Option<StoreItem>)>, Option<String>);

impl CortexEngine {
    /// See the module docs. An item that cannot be assembled whole is left
    /// out.
    pub(super) async fn list_page(&self, req: ListRequest) -> Result<ListPage> {
        let (items, next_cursor) = self.items_page(req, false).await?;
        Ok(ListPage {
            items: items
                .into_iter()
                .filter_map(|(id, item)| item.map(|item| hit(&id, &item, 0.0)))
                .collect(),
            next_cursor,
        })
    }

    /// The same walk as [`Self::list_page`], handing each item back whole;
    /// an item that cannot be assembled whole is named, not left out.
    /// Every scope must be listable: an export that silently missed some
    /// would move part of the memory and report it all moved.
    ///
    /// # Errors
    ///
    /// The listing's: an invalid request or cursor, a scope or event page the
    /// engine fails to answer, and a walk past its page cap. Nothing is
    /// returned partially on an error; the caller retries from its cursor.
    pub(super) async fn export_page(&self, req: ListRequest) -> Result<ExportPage> {
        let (items, next_cursor) = self.items_page(req, true).await?;
        let mut page = ExportPage {
            next_cursor,
            ..ExportPage::default()
        };
        for (id, item) in items {
            match item {
                Some(item) => page.items.push(Exported {
                    id: ItemId::new(id),
                    item,
                }),
                None => page.incomplete.push(ItemId::new(id)),
            }
        }
        Ok(page)
    }

    /// One page of the walk the module docs describe; `complete` refuses
    /// when the engine cannot list every scope.
    async fn items_page(&self, req: ListRequest, complete: bool) -> Result<ItemsPage> {
        req.validate()?;
        let scopes = if complete {
            self.all_scopes_for(&req.filter).await?
        } else {
            self.scopes_for(&req.filter).await?
        };
        if scopes.is_empty() {
            return Ok((Vec::new(), None));
        }
        let mut at = match &req.cursor {
            Some(raw) => cursor::decode::<ListCursor>(TAG, raw)?,
            None => ListCursor::at(&scopes[0].path),
        };
        let start = resume_at(&scopes, &mut at);
        let narrowing = labels::narrowing(&req.filter);
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        let mut next = None;
        // The cap guards against a cursor that never ends, for the whole
        // request. Every scope gets one page on top of it: an empty scope (a
        // registration left after a forget) costs a page of its own, so a
        // bare cap would refuse a store with many of them while every cursor
        // was ending. The scope listing bounds how many there are.
        let budget = MAX_PAGES + scopes.len();
        let mut pages = 0;
        'scopes: for (index, scope) in scopes.iter().enumerate().skip(start) {
            let kind = scope.kind;
            if index > start || at.scope.as_deref() != Some(scope.path.as_str()) {
                at = ListCursor::at(&scope.path);
            }
            loop {
                pages += 1;
                if pages > budget {
                    return Err(Error::Engine(format!(
                        "listing read {budget} pages (stopped in {}) without filling a page of \
                         results; refusing to walk further",
                        scope.path
                    )));
                }
                let page = self
                    .log
                    .page(
                        &scope.path,
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
                                && index + 1 == scopes.len();
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
        Ok((self.resolve(pending).await?, next))
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
        let starts = envelope.part().is_none_or(|index| index == 0);
        if !starts || !seen.insert(envelope.id.clone()) {
            return None;
        }
        if kind == ItemKind::Conversation || envelope.chunk.is_some() {
            return Some(Pending::Assembled(
                kind,
                envelope.id,
                envelope.meta.namespace,
            ));
        }
        let item = rebuild(std::slice::from_ref::<Envelope>(&envelope))?;
        Some(Pending::Ready(envelope.id, Box::new(item)))
    }

    /// Assembles the page's conversations and chunked documents (one lookup
    /// per kind) and returns the items in listing order.
    async fn resolve(&self, pending: Vec<Pending>) -> Result<Vec<(String, Option<StoreItem>)>> {
        let mut assembled = HashMap::new();
        for kind in [ItemKind::Conversation, ItemKind::Document] {
            let ids: Vec<(String, Namespace)> = pending
                .iter()
                .filter_map(|p| match p {
                    Pending::Assembled(of, id, namespace) if *of == kind => {
                        Some((id.clone(), namespace.clone()))
                    }
                    _ => None,
                })
                .collect();
            if !ids.is_empty() {
                assembled.extend(self.assembled(kind, &ids).await?);
            }
        }
        Ok(pending
            .into_iter()
            .map(|p| match p {
                Pending::Ready(id, item) => (id, Some(*item)),
                Pending::Assembled(_, id, _) => {
                    let item = assembled.remove(&id);
                    (id, item)
                }
            })
            .collect())
    }
}

/// Where in `scopes` a listing at `at` resumes. The cursor's scope is found
/// by path; one that no longer exists resumes at the next scope in order,
/// from its first page.
fn resume_at(scopes: &[KindScope], at: &mut ListCursor) -> usize {
    let Some(path) = at.scope.clone() else {
        return 0;
    };
    if let Some(index) = scopes.iter().position(|scope| scope.path == path) {
        return index;
    }
    *at = ListCursor::default();
    let Some((namespace, kind)) = parse_scope(&path) else {
        return scopes.len();
    };
    let gone = KindScope::new(namespace, kind);
    scopes
        .iter()
        .position(|scope| *scope > gone)
        .unwrap_or(scopes.len())
}
