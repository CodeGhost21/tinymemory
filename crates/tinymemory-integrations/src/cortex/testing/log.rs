//! An in-memory CortexDB event log with the engine's measured quirks.
//!
//! Deliberately unaccommodating, because a tidy double proves nothing:
//!
//! - append-only, with the body `idempotency_key` remembered (a reused key
//!   with a different body is `409 IDEMPOTENCY_CONFLICT`, the same body is
//!   a replay); forgetting an event by `memory_ids` releases its key, as
//!   CortexDB 0.10.4 does;
//! - the listing is newest first and emits **every event twice**, with
//!   `limit` counting the copies;
//! - unknown query parameters are ignored;
//! - the forget selector reads only `memory_ids`; an empty selector without
//!   `confirm_all` is refused, and a selector with `confirm_all` is refused
//!   as ambiguous;
//! - recall renders text for a reader (`[role] {...}`), honours `view:
//!   "descend"`, metadata label filters and the events budget;
//! - a belief build turns each of a scope's events into one supported
//!   belief (`user said <text>`), which recall returns in `layers.beliefs`
//!   when its budget asks for them, and `GET /v1/beliefs` lists.

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
    /// Beliefs built, oldest first.
    pub(crate) beliefs: Vec<Value>,
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
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

impl CortexLog {
    /// `POST /v1/experience`: (status, body). Like CortexDB 0.10.4, an
    /// event over 1 MiB of text is refused (`422 INVALID_ENVELOPE`).
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
        if text.len() > 1024 * 1024 {
            return (422, json!({ "error_code": "INVALID_ENVELOPE" }));
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

    /// `GET /v1/scopes/list`: every scope holding an event, at or below
    /// `prefix` on a segment boundary, sorted.
    pub(crate) fn scopes(&self, prefix: &str) -> Vec<String> {
        let below = format!("{prefix}/");
        let mut scopes: Vec<String> = self
            .events
            .iter()
            .map(|e| str_of(e, "/scope").to_string())
            .filter(|scope| prefix.is_empty() || scope == prefix || scope.starts_with(&below))
            .collect();
        scopes.sort();
        scopes.dedup();
        scopes
    }

    /// `GET /v1/events`: newest first, every event twice, `limit` counting
    /// the copies, an offset `cursor`.
    pub(crate) fn page(&self, params: &BTreeMap<String, String>) -> Value {
        let scope = params.get("scope").cloned().unwrap_or_default();
        let cursor: usize = params
            .get("cursor")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let limit: usize = params
            .get("limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);
        let wanted: Vec<&str> = params
            .get("labels")
            .map(|l| {
                l.split(',')
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect()
            })
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
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let selective = !ids.is_empty();
        if selective && confirm_all {
            return (
                400,
                json!({ "error_code": "AMBIGUOUS_SELECTOR_CONFIRM_ALL" }),
            );
        }
        if !selective && !confirm_all {
            return (
                422,
                json!({ "error_code": "EMPTY_SELECTOR_WITHOUT_CONFIRMATION" }),
            );
        }
        let before = self.events.len();
        if selective {
            let (gone, kept): (Vec<Value>, Vec<Value>) =
                std::mem::take(&mut self.events).into_iter().partition(|e| {
                    str_of(e, "/scope") == scope && ids.iter().any(|id| id == str_of(e, "/id"))
                });
            self.forgotten
                .extend(gone.iter().map(|e| str_of(e, "/id").to_string()));
            self.idempotency
                .retain(|_, (_, id)| !gone.iter().any(|e| str_of(e, "/id") == id));
            self.events = kept;
        } else {
            self.events.retain(|e| str_of(e, "/scope") != scope);
        }
        let deleted = before - self.events.len();
        (
            200,
            json!({ "deleted": { "events": deleted }, "requested": ids.len() }),
        )
    }

    /// Removes the last event of `scope` and its idempotency record, as if
    /// it had never been written (a store that failed part way).
    pub(crate) fn lose_last(&mut self, scope: &str) {
        let Some(at) = self
            .events
            .iter()
            .rposition(|e| str_of(e, "/scope") == scope)
        else {
            return;
        };
        let lost = self.events.remove(at);
        let id = str_of(&lost, "/id").to_string();
        self.idempotency.retain(|_, (_, held)| *held != id);
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
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        // A pack's event carries its stored text: the `[role] ` marker is
        // only in `context_block` and the index copy (0.10.3/0.10.4 API
        // §9.4), which this double does not render.
        let events: Vec<Value> = scored
            .into_iter()
            .take(budget)
            .map(|(_, hit)| hit)
            .collect();
        let wanted = body
            .pointer("/budgets/per_layer_limits/beliefs")
            .and_then(Value::as_u64)
            .map_or(0, |b| usize::try_from(b).unwrap_or(usize::MAX));
        let beliefs: Vec<Value> = self
            .beliefs
            .iter()
            .rev()
            .filter(|belief| in_scope(belief))
            .filter(|belief| {
                let text = belief.to_string().to_lowercase();
                words.is_empty() || words.iter().any(|w| text.contains(w.as_str()))
            })
            .take(wanted)
            .cloned()
            .collect();
        json!({ "pack_id": "pack_test", "layers": { "events": events, "beliefs": beliefs } })
    }

    /// `POST /v1/beliefs/build`: one belief per event in `scope` that has
    /// none yet; how many were built.
    pub(crate) fn build(&mut self, scope: &str) -> usize {
        let fresh: Vec<Value> = self
            .events
            .iter()
            .filter(|event| str_of(event, "/scope") == scope)
            .filter(|event| {
                let id = str_of(event, "/id");
                !self.beliefs.iter().any(|b| str_of(b, "/source") == id)
            })
            .map(|event| {
                let raw = str_of(event, "/content/text");
                let said = serde_json::from_str::<Value>(raw)
                    .ok()
                    .and_then(|envelope| envelope["text"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| raw.to_string());
                json!({
                    "id": format!("belief_{}", str_of(event, "/id")),
                    "source": str_of(event, "/id"),
                    "scope": scope,
                    "claim": {
                        "subject": { "type": "entity", "id": "ent_user", "name": "user" },
                        "predicate": "said",
                        "object": { "type": "literal", "datatype": "string", "value": said },
                    },
                    "stance": "supported",
                    "confidence": 0.9,
                    "valid_from": "2026-09-01T09:00:00Z",
                })
            })
            .collect();
        let built = fresh.len();
        self.beliefs.extend(fresh);
        built
    }

    /// `GET /v1/beliefs?scope=`: the scope's beliefs, newest first.
    pub(crate) fn list_beliefs(&self, scope: &str) -> Vec<Value> {
        self.beliefs
            .iter()
            .rev()
            .filter(|belief| str_of(belief, "/scope") == scope)
            .cloned()
            .collect()
    }
}
