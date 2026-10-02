//! An in-memory CortexDB event log with the engine's measured quirks.
//!
//! Deliberately unaccommodating, because a tidy double proves nothing:
//!
//! - append-only, with the body `idempotency_key` remembered for ever (a
//!   reused key with a different body is `409 IDEMPOTENCY_CONFLICT`, the
//!   same body is a replay, and forgetting an event does not release it);
//! - the listing is newest first and emits **every event twice**, with
//!   `limit` counting the copies;
//! - unknown query parameters are ignored;
//! - the forget selector reads only `memory_ids`; an empty selector without
//!   `confirm_all` is refused, and a selector with `confirm_all` is refused
//!   as ambiguous;
//! - recall renders text for a reader (`[role] {...}`), honours `view:
//!   "descend"`, metadata label filters and the events budget.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The log.
#[derive(Debug, Default)]
pub(crate) struct CortexLog {
    /// Every event appended and not forgotten, oldest first.
    pub(crate) events: Vec<Value>,
    /// `idempotency_key` → (body text, event id).
    idempotency: BTreeMap<String, (String, String)>,
    next_id: u64,
    /// Every event id a forget removed.
    pub(crate) forgotten: Vec<String>,
}

/// Whether `event` carries any one of `wanted` (an empty list keeps all).
fn labelled(event: &Value, wanted: &[&str]) -> bool {
    wanted.is_empty()
        || event
            .pointer("/context/labels")
            .and_then(Value::as_array)
            .is_some_and(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|label| wanted.contains(&label))
            })
}

fn str_of<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or_default()
}

impl CortexLog {
    /// `POST /v1/experience`: (status, body).
    pub(crate) fn append(&mut self, body: &Value) -> (u16, Value) {
        let key = str_of(body, "/idempotency_key").to_string();
        let text = str_of(body, "/content/text").to_string();
        if let Some((seen, id)) = self.idempotency.get(&key) {
            if seen != &text {
                return (409, json!({ "error_code": "IDEMPOTENCY_CONFLICT" }));
            }
            return (
                202,
                json!({ "event_id": id, "replayed_from_idempotency": true }),
            );
        }
        self.next_id += 1;
        let id = format!("evt_{}", self.next_id);
        self.idempotency.insert(key, (text, id.clone()));
        let mut context = body
            .get("context")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        context["recorded_at"] = json!("2026-09-02T00:00:00Z");
        self.events.push(json!({
            "id": id,
            "scope": str_of(body, "/scope"),
            "modality": str_of(body, "/modality"),
            "content": body.get("content").cloned().unwrap_or_default(),
            "context": context,
        }));
        (
            202,
            json!({ "event_id": id, "status": "captured", "replayed_from_idempotency": false }),
        )
    }

    /// `GET /v1/events`: newest first, every event twice, `limit` counting
    /// the copies, an offset `cursor`.
    pub(crate) fn page(&self, params: &BTreeMap<String, String>) -> Value {
        let scope = params.get("scope").cloned().unwrap_or_default();
        let cursor: usize = params.get("cursor").and_then(|v| v.parse().ok()).unwrap_or(0);
        let limit: usize = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);
        let wanted: Vec<&str> = params
            .get("labels")
            .map(|l| l.split(',').map(str::trim).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default();
        let mut stream = Vec::new();
        for event in self
            .events
            .iter()
            .rev()
            .filter(|e| str_of(e, "/scope") == scope && labelled(e, &wanted))
        {
            stream.push(event.clone());
            stream.push(event.clone());
        }
        let page: Vec<Value> = stream.iter().skip(cursor).take(limit).cloned().collect();
        let next = cursor + page.len();
        json!({ "items": page, "has_more": next < stream.len(), "next_cursor": next.to_string() })
    }

    /// `POST /v1/forget`, with the real interlocks.
    pub(crate) fn forget(&mut self, body: &Value) -> (u16, Value) {
        let scope = str_of(body, "/scope").to_string();
        let confirm_all = body
            .get("confirm_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ids: Vec<String> = body
            .pointer("/selector/memory_ids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let selective = !ids.is_empty();
        if selective && confirm_all {
            return (400, json!({ "error_code": "AMBIGUOUS_SELECTOR_CONFIRM_ALL" }));
        }
        if !selective && !confirm_all {
            return (422, json!({ "error_code": "EMPTY_SELECTOR_WITHOUT_CONFIRMATION" }));
        }
        let before = self.events.len();
        if selective {
            let (gone, kept): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.events)
                .into_iter()
                .partition(|e| str_of(e, "/scope") == scope && ids.iter().any(|id| id == str_of(e, "/id")));
            self.forgotten
                .extend(gone.iter().map(|e| str_of(e, "/id").to_string()));
            self.events = kept;
        } else {
            self.events.retain(|e| str_of(e, "/scope") != scope);
        }
        let deleted = before - self.events.len();
        (200, json!({ "deleted": { "events": deleted }, "requested": ids.len() }))
    }

    /// `POST /v1/recall`: events ranked by how many query words they hold.
    pub(crate) fn recall(&self, body: &Value) -> Value {
        let scope = str_of(body, "/scope");
        let descend = body.get("view").and_then(Value::as_str) == Some("descend");
        let in_scope = |event: &Value| {
            let held = str_of(event, "/scope");
            held == scope || (descend && held.starts_with(&format!("{scope}/")))
        };
        let wanted: Vec<&str> = body
            .pointer("/filters/metadata/labels")
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let words: Vec<String> = str_of(body, "/query")
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| w.len() >= 3)
            .collect();
        let budget = body
            .pointer("/budgets/per_layer_limits/events")
            .and_then(Value::as_u64)
            .map_or(usize::MAX, |b| usize::try_from(b).unwrap_or(usize::MAX));
        let mut scored: Vec<(usize, Value)> = self
            .events
            .iter()
            .rev()
            .filter(|e| in_scope(e) && labelled(e, &wanted))
            .filter_map(|e| {
                let text = str_of(e, "/content/text").to_lowercase();
                let score = words.iter().filter(|w| text.contains(w.as_str())).count();
                (words.is_empty() || score > 0).then(|| (score, e.clone()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        let events: Vec<Value> = scored
            .into_iter()
            .take(budget)
            .map(|(_, mut hit)| {
                let role = str_of(&hit, "/content/role").to_string();
                let text = str_of(&hit, "/content/text").to_string();
                hit["content"]["text"] = json!(format!("[{role}] {text}"));
                hit
            })
            .collect();
        json!({ "pack_id": "pack_test", "layers": { "events": events } })
    }
}
