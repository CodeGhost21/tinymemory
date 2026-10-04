//! [`ReferenceEngine`]: an in-memory engine whose behaviour is obvious by
//! inspection.
//!
//! It is the suite's calibration subject: a failure against it means the
//! assertion is wrong, not the engine. It serves every fetch mode, using a
//! trivial keyword scorer and a deterministic toy vector (see `score`), and
//! answers recall by quoting its best hybrid hits.

mod score;

use std::sync::Mutex;

use crate::{
    Citation, EngineDescriptor, EngineHealth, Error, FetchMode, FetchPage, FetchRequest,
    ForgetReport, ForgetTarget, Hit, ItemId, ListPage, ListRequest, MemoryEngine, MetaFilter,
    RecallAnswer, RecallRequest, Result, StoreItem, StoreReceipt,
};
use async_trait::async_trait;

/// The reference engine's id.
pub const REFERENCE_ENGINE_ID: &str = "reference";

/// An in-memory [`MemoryEngine`] serving every fetch mode.
#[derive(Debug)]
pub struct ReferenceEngine {
    descriptor: EngineDescriptor,
    items: Mutex<Vec<StoreItem>>,
}

impl Default for ReferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceEngine {
    /// An empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            descriptor: EngineDescriptor {
                id: REFERENCE_ENGINE_ID,
                label: "Reference (in-memory)",
                description: "An in-memory engine with toy keyword and vector scoring, for tests.",
                hosted: false,
                needs_endpoint: false,
                needs_key: false,
                default_endpoint: None,
                fetch_modes: FetchMode::ALL.to_vec(),
                consolidation: CONS,
            },
            items: Mutex::new(Vec::new()),
        }
    }

    /// How many items the engine holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items().map(|items| items.len()).unwrap_or_default()
    }

    /// Whether the engine holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn items(&self) -> Result<std::sync::MutexGuard<'_, Vec<StoreItem>>> {
        self.items
            .lock()
            .map_err(|_| Error::Engine("reference engine state is poisoned".to_string()))
    }

    /// Hits for a validated fetch, best first, before paging.
    fn ranked(&self, query: &str, mode: FetchMode, filter: &MetaFilter) -> Result<Vec<Hit>> {
        let items = self.items()?;
        let mut hits: Vec<(usize, Hit)> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| filter.matches(item.kind(), item.meta()))
            .filter_map(|(order, item)| {
                let text = item.render_text();
                score::score(mode, query, &text).map(|score| (order, hit(item, text, score)))
            })
            .collect();
        hits.sort_by(|(a_order, a), (b_order, b)| {
            b.score.total_cmp(&a.score).then(a_order.cmp(b_order))
        });
        Ok(hits.into_iter().map(|(_, hit)| hit).collect())
    }
}

fn hit(item: &StoreItem, text: String, score: f32) -> Hit {
    Hit {
        id: ItemId(item.fingerprint()),
        kind: item.kind(),
        text,
        meta: item.meta().clone(),
        score,
        confidence: item.confidence(),
    }
}

/// Splits `all` into the page starting at `cursor` and the next cursor.
fn page<T>(all: Vec<T>, cursor: Option<&str>, limit: usize) -> Result<(Vec<T>, Option<String>)> {
    let start = match cursor {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| Error::InvalidRequest(format!("unknown cursor `{cursor}`")))?,
        None => 0,
    };
    let total = all.len();
    let items: Vec<T> = all.into_iter().skip(start).take(limit).collect();
    let next = start + items.len();
    Ok((items, (next < total).then(|| next.to_string())))
}

#[async_trait]
impl MemoryEngine for ReferenceEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    async fn health(&self) -> EngineHealth {
        match self.items() {
            Ok(_) => EngineHealth::Ok,
            Err(error) => EngineHealth::Down(error.to_string()),
        }
    }

    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer> {
        req.validate()?;
        let hits = self.ranked(&req.question, FetchMode::Hybrid, &req.filter)?;
        let citations: Vec<Citation> = hits
            .into_iter()
            .take(req.limit)
            .map(|hit| Citation {
                id: hit.id,
                kind: hit.kind,
                snippet: hit.text,
                meta: hit.meta,
                score: Some(hit.score),
            })
            .collect();
        let answer = if citations.is_empty() {
            "Nothing stored answers this question.".to_string()
        } else {
            let quoted: Vec<&str> = citations.iter().map(|c| c.snippet.as_str()).collect();
            format!(
                "From {} stored items: {}",
                citations.len(),
                quoted.join(" | ")
            )
        };
        Ok(RecallAnswer {
            answer,
            citations,
            model: Some(REFERENCE_ENGINE_ID.to_string()),
        })
    }

    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage> {
        self.descriptor.ensure_mode(req.mode)?;
        req.validate()?;
        let hits = self.ranked(&req.query, req.mode, &req.filter)?;
        let (hits, next_cursor) = page(hits, req.cursor.as_deref(), req.limit)?;
        Ok(FetchPage { hits, next_cursor })
    }

    async fn store(&self, item: StoreItem) -> Result<StoreReceipt> {
        item.validate()?;
        let id = ItemId(item.fingerprint());
        let mut items = self.items()?;
        let replayed = items.iter().any(|held| held.fingerprint() == id.0);
        if !replayed {
            items.push(item);
        }
        Ok(StoreReceipt { id, replayed })
    }

    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport> {
        target.validate()?;
        let mut items = self.items()?;
        let before = items.len();
        match &target {
            ForgetTarget::Ids(ids) => {
                items.retain(|item| !ids.iter().any(|id| id.0 == item.fingerprint()));
            }
            ForgetTarget::Filter(filter) => {
                items.retain(|item| !filter.matches(item.kind(), item.meta()));
            }
        }
        Ok(ForgetReport {
            forgotten: before - items.len(),
        })
    }

    async fn list(&self, req: ListRequest) -> Result<ListPage> {
        req.validate()?;
        let matching: Vec<Hit> = self
            .items()?
            .iter()
            .filter(|item| req.filter.matches(item.kind(), item.meta()))
            .map(|item| hit(item, item.render_text(), 0.0))
            .collect();
        let (items, next_cursor) = page(matching, req.cursor.as_deref(), req.limit)?;
        Ok(ListPage { items, next_cursor })
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
