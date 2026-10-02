//! What the server understood from what was written, as the summary forest
//! the Brain view draws.
//!
//! The embedded engine seals a summary tree on the device: hourly, daily and
//! monthly summaries over the chunks they cover. Hosted memory seals nothing.
//! The server derives three layers from every scope written to instead, each
//! linked to what it came from:
//!
//! - **facts** — subject, predicate, object — cite the events that support
//!   them;
//! - **beliefs** — claims consolidated from facts and events — list their
//!   support, each entry typed and weighted;
//! - **concepts** — the server's understanding — list the beliefs and facts
//!   that support them.
//!
//! So the forest is those layers, stacked as the contract stacks seal
//! generations: facts at level 1 over the events they cite, beliefs at level 2
//! over their facts, concepts at level 3 over their beliefs and facts. Each
//! node hangs under the one node a level up that cites it — a fact under a
//! belief before a concept — chosen by the citer's confidence, and each event
//! leaf under the fact that cites it. A node's children are the ones hanging
//! under it, so the forest stays a tree. Its text travels as `preview`,
//! because there is no file in a content vault to read it from.
//!
//! # Which namespaces
//!
//! [`DERIVED_NAMESPACES`]: the default namespace notes land in, and the three
//! the source sink writes. Their derivation stands for the account's; reading
//! every namespace the user ever named would spend the backend's rate limit
//! on scopes the Brain view does not draw.
//!
//! # What is left out
//!
//! What the server has set aside: a fact another superseded, a belief or
//! concept it deprecated, a concept merged into another, and anything struck
//! from the record. Episodes have no route on the backend, so a concept's
//! support by episodes is not drawn.
//!
//! # Scope and cost
//!
//! A derived node draws on events from any source, so it cannot be shown to a
//! caller confined to some of them: a read under a source scope answers no
//! derived nodes. The layer routes are not billed but count against the
//! backend's rate limit, so one reading — four namespaces, three layers, a
//! page or more each — is reused for a minute, which covers the forest and the
//! leaves the Brain view reads straight after it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::Value;
use tinymemory_api::chrono::{DateTime, Utc};
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::tree::{leaf_preview, TreeSummary};
use tinymemory_api::types::GLOBAL_NAMESPACE;

use super::sources::namespace_of as source_namespace;
use crate::common::Attempts;
use crate::cortex::{urlencoding, Route};
use crate::cortex_provider::CortexProvider;
use tinymemory_api::chunks::SourceKind;

/// How long one reading of the layers answers every read.
pub(crate) const FOREST_TTL: Duration = Duration::from_secs(60);

/// Items asked for per page of a layer.
const LAYER_PAGE: usize = 200;

/// Most items one layer is read to, per namespace. A layer cut here makes the
/// forest truncated.
const MAX_LAYER_ITEMS: usize = 2_000;

/// Most children one node lists.
const MAX_CHILDREN: usize = 64;

/// The kind every derived tree reports.
pub(super) const TREE_KIND: &str = "understanding";

/// The namespaces whose derived layers make the forest.
pub(super) fn derived_namespaces() -> [&'static str; 4] {
    [
        GLOBAL_NAMESPACE,
        source_namespace(SourceKind::Chat),
        source_namespace(SourceKind::Email),
        source_namespace(SourceKind::Document),
    ]
}

/// The server's derived layers for every namespace, as one forest.
#[derive(Debug, Default)]
pub(super) struct Forest {
    /// Tree by tree, level by level, oldest first.
    pub(super) summaries: Vec<TreeSummary>,
    /// Each event's parent: the fact that cites it or, failing one, the belief.
    pub(super) leaf_parents: HashMap<String, String>,
    /// Whether a layer was cut at [`MAX_LAYER_ITEMS`].
    pub(super) truncated: bool,
}

/// The last reading, shared by every read within its time to live.
#[derive(Debug)]
pub(crate) struct ForestCache {
    ttl: Duration,
    last: Mutex<Option<(Instant, Arc<Forest>)>>,
}

impl ForestCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            last: Mutex::new(None),
        }
    }

    fn fresh(&self) -> Option<Arc<Forest>> {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(read_at, _)| read_at.elapsed() < self.ttl)
            .map(|(_, forest)| Arc::clone(forest))
    }

    fn keep(&self, forest: &Arc<Forest>) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Instant::now(), Arc::clone(forest)));
    }
}

/// What a derived node cites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cited {
    Event,
    Fact,
    Belief,
}

/// One derived node before it is placed.
#[derive(Debug)]
struct Node {
    id: String,
    level: u32,
    text: String,
    confidence: f64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    /// What it cites, strongest first.
    cites: Vec<(Cited, String)>,
}

fn text_of<'a>(item: &'a Value, field: &str) -> Option<&'a str> {
    item.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn time_of(item: &Value, field: &str) -> Option<DateTime<Utc>> {
    text_of(item, field)
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .map(|stamp| stamp.with_timezone(&Utc))
}

/// When a node holds from, and until the latest of `until`, never before it
/// starts.
fn span(item: &Value, until: &[&str]) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = time_of(item, "valid_from")
        .or_else(|| time_of(item, "recorded_from"))
        .unwrap_or_default();
    let end = until
        .iter()
        .filter_map(|field| time_of(item, field))
        .max()
        .unwrap_or(start)
        .max(start);
    (start, end)
}

/// Whether the server has set `item` aside: struck from the record,
/// deprecated, superseded by another, or merged into another.
fn set_aside(item: &Value) -> bool {
    let named = |field: &str| item.get(field).is_some_and(|value| !value.is_null());
    named("recorded_to")
        || named("superseded_by")
        || named("canonical_id")
        || text_of(item, "stance") == Some("deprecated")
}

/// A typed value as words: an entity or concept by name, a literal as itself.
fn value_text(value: &Value) -> Option<String> {
    match text_of(value, "type")? {
        "entity" | "concept" => text_of(value, "name")
            .or_else(|| text_of(value, "id"))
            .map(str::to_string),
        "literal" => match value.get("value")? {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Null => None,
            other => Some(other.to_string()),
        },
        _ => None,
    }
    .filter(|text| !text.is_empty())
}

/// A subject–predicate–object claim as one sentence.
fn claim_text(claim: &Value) -> Option<String> {
    let subject = value_text(claim.get("subject")?)?;
    let predicate = text_of(claim, "predicate")?.replace('_', " ");
    let object = value_text(claim.get("object")?)?;
    Some(format!("{subject} {predicate} {object}"))
}

fn confidence(item: &Value) -> f64 {
    item.get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

fn ids(value: Option<&Value>) -> impl Iterator<Item = String> + '_ {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn fact(item: &Value) -> Option<Node> {
    if set_aside(item) {
        return None;
    }
    let (start, end) = span(item, &["valid_to", "recorded_from"]);
    Some(Node {
        id: text_of(item, "id")?.to_string(),
        level: 1,
        text: claim_text(item)?,
        confidence: confidence(item),
        start,
        end,
        cites: ids(item.get("supports"))
            .map(|id| (Cited::Event, id))
            .collect(),
    })
}

fn belief(item: &Value) -> Option<Node> {
    if set_aside(item) {
        return None;
    }
    let claim = claim_text(item.get("claim")?)?;
    let text = match text_of(item, "stance") {
        Some("contradicted") => format!("{claim} (contradicted)"),
        Some("uncertain") => format!("{claim} (uncertain)"),
        _ => claim,
    };
    let mut support: Vec<(f64, Cited, String)> = item
        .get("supports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| text_of(entry, "polarity") != Some("against"))
        .filter_map(|entry| {
            let cited = match text_of(entry, "type")? {
                "event" => Cited::Event,
                "fact" => Cited::Fact,
                "belief" => Cited::Belief,
                _ => return None,
            };
            let weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
            Some((weight, cited, text_of(entry, "id")?.to_string()))
        })
        .collect();
    support.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (start, end) = span(item, &["last_revised_at", "valid_to"]);
    Some(Node {
        id: text_of(item, "id")?.to_string(),
        level: 2,
        text,
        confidence: confidence(item),
        start,
        end,
        cites: support
            .into_iter()
            .map(|(_, cited, id)| (cited, id))
            .collect(),
    })
}

fn concept(item: &Value) -> Option<Node> {
    if set_aside(item) {
        return None;
    }
    let name = text_of(item, "name")?;
    let text = match text_of(item, "summary") {
        Some(summary) if summary != name => format!("{name} — {summary}"),
        _ => name.to_string(),
    };
    let supported_by = item.get("supported_by");
    let cites = ids(supported_by.and_then(|by| by.get("beliefs")))
        .map(|id| (Cited::Belief, id))
        .chain(ids(supported_by.and_then(|by| by.get("facts"))).map(|id| (Cited::Fact, id)))
        .collect();
    let (start, end) = span(item, &["last_synthesized_at", "valid_to"]);
    Some(Node {
        id: text_of(item, "id")?.to_string(),
        level: 3,
        text,
        confidence: confidence(item),
        start,
        end,
        cites,
    })
}

/// Places one namespace's layers in `forest`.
///
/// A fact hangs under the most confident belief that cites it, else the most
/// confident concept; a belief under the most confident concept; an event
/// under the most confident fact, else belief. First claim wins, so the
/// forest stays a tree.
fn plant(forest: &mut Forest, namespace: &str, mut layers: [Vec<Node>; 3]) {
    for layer in &mut layers {
        layer.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    }
    let [facts, beliefs, concepts] = layers;
    let fact_ids: HashSet<&str> = facts.iter().map(|node| node.id.as_str()).collect();
    let belief_ids: HashSet<&str> = beliefs.iter().map(|node| node.id.as_str()).collect();

    let mut parents: HashMap<String, String> = HashMap::new();
    let mut claim = |citers: &[Node], wanted: &dyn Fn(Cited, &str) -> bool| {
        for citer in citers {
            for (cited, id) in &citer.cites {
                if wanted(*cited, id) {
                    parents
                        .entry(id.clone())
                        .or_insert_with(|| citer.id.clone());
                }
            }
        }
    };
    claim(&beliefs, &|cited, id| {
        cited == Cited::Fact && fact_ids.contains(id)
    });
    claim(&concepts, &|cited, id| match cited {
        Cited::Belief => belief_ids.contains(id),
        Cited::Fact => fact_ids.contains(id),
        Cited::Event => false,
    });
    let mut leaves: HashMap<String, String> = HashMap::new();
    for citer in facts.iter().chain(&beliefs) {
        for (cited, id) in &citer.cites {
            if *cited == Cited::Event && !forest.leaf_parents.contains_key(id) {
                leaves.entry(id.clone()).or_insert_with(|| citer.id.clone());
            }
        }
    }

    let mut children: HashMap<&str, Vec<String>> = HashMap::new();
    for (child, parent) in parents.iter().chain(&leaves) {
        children
            .entry(parent.as_str())
            .or_default()
            .push(child.clone());
    }
    // Level by level, oldest first; stable, so a tie keeps the more confident
    // node first.
    let mut nodes: Vec<Node> = facts.into_iter().chain(beliefs).chain(concepts).collect();
    nodes.sort_by(|a, b| a.level.cmp(&b.level).then(a.start.cmp(&b.start)));
    for node in nodes {
        let mut child_ids = children.remove(node.id.as_str()).unwrap_or_default();
        child_ids.sort();
        child_ids.truncate(MAX_CHILDREN);
        forest.summaries.push(TreeSummary {
            parent_id: parents.get(&node.id).cloned(),
            tree_id: format!("hosted:{namespace}"),
            tree_kind: TREE_KIND.to_string(),
            tree_scope: namespace.to_string(),
            level: node.level,
            child_ids,
            time_range_start: node.start,
            time_range_end: node.end,
            preview: Some(leaf_preview(&node.text)),
            id: node.id,
        });
    }
    forest.leaf_parents.extend(leaves);
}

impl CortexProvider {
    /// The server's layers as a forest, reusing a reading made within the
    /// cache's time to live.
    pub(super) async fn forest(&self) -> Result<Arc<Forest>, MemoryError> {
        if let Some(forest) = self.families.forest.fresh() {
            return Ok(forest);
        }
        let forest = Arc::new(self.read_forest().await?);
        self.families.forest.keep(&forest);
        Ok(forest)
    }

    async fn read_forest(&self) -> Result<Forest, MemoryError> {
        let mut forest = Forest::default();
        for namespace in derived_namespaces() {
            let scope = self.dialect.scope_for(namespace).map_err(engine_error)?;
            let mut layers: [Vec<Node>; 3] = Default::default();
            for (slot, (route, parse)) in [
                (Route::Facts, fact as fn(&Value) -> Option<Node>),
                (Route::Beliefs, belief),
                (Route::Understanding, concept),
            ]
            .into_iter()
            .enumerate()
            {
                let (items, complete) = self.read_layer(route, &scope).await?;
                forest.truncated |= !complete;
                layers[slot] = items.iter().filter_map(parse).collect();
            }
            plant(&mut forest, namespace, layers);
        }
        Ok(forest)
    }

    /// Every item of one layer in `scope`, page by page, up to
    /// [`MAX_LAYER_ITEMS`]; `false` when that cut it short.
    async fn read_layer(
        &self,
        route: Route,
        scope: &str,
    ) -> Result<(Vec<Value>, bool), MemoryError> {
        let base = self.dialect.wire.path(route);
        let mut items: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("{base}?scope={}&limit={LAYER_PAGE}", urlencoding(scope));
            if let Some(cursor) = &cursor {
                path.push_str("&cursor=");
                path.push_str(&urlencoding(cursor));
            }
            let page: Value = self
                .dialect
                .client
                .json(Method::GET, &path, None, Attempts::RetryTransient)
                .await
                .map_err(engine_error)?;
            items.extend(
                page.get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let more = page.get("has_more").and_then(Value::as_bool) == Some(true);
            cursor = text_of(&page, "next_cursor").map(str::to_string);
            if !more || cursor.is_none() {
                return Ok((items, true));
            }
            if items.len() >= MAX_LAYER_ITEMS {
                items.truncate(MAX_LAYER_ITEMS);
                return Ok((items, false));
            }
        }
    }
}

#[cfg(test)]
#[path = "understanding_tests.rs"]
mod test;
