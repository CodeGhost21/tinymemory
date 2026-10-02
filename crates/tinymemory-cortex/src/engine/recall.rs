//! Recall: one recall pack, then the answer route once with that pack.
//!
//! When the filter admits exactly one kind the pack is built over that kind's
//! scope; otherwise over the TinyMemory root with `view: "descend"`, which
//! recalls the root and every scope under it. The answer route is then asked
//! once with `use_pack_id`, so it answers from exactly the evidence the pack
//! holds.
//!
//! Citations come from the pack's `layers.events`, decoded back to items,
//! filtered by the full [`tinymemory_api::MetaFilter`], one per item, at most
//! `limit`. CortexDB scores none of them. A pack with no decodable events
//! still returns the engine's answer, with no citations.

use serde_json::{Value, json};
use tinymemory_api::{Citation, ItemId, RecallAnswer, RecallRequest};

use super::CortexEngine;
use super::fetch::{ranked, recall_body};
use super::items::admitted;
use crate::descriptor::CortexWire;
use crate::envelope::{ROOT_SCOPE, scope_of};
use crate::error::{Error, Result};

/// The derived layers a pack also draws on, besides events.
const DERIVED_LAYERS: [&str; 4] = ["facts", "beliefs", "episodes", "understanding"];

/// Per-layer budgets for a pack answering with at most `limit` citations:
/// twice that many events (a conversation contributes several turns, and the
/// filter drops some), and `limit` shared across the derived layers.
fn pack_budgets(limit: usize) -> Value {
    let mut layers = serde_json::Map::new();
    layers.insert("events".to_string(), json!(limit.saturating_mul(2)));
    let base = limit / DERIVED_LAYERS.len();
    let remainder = limit % DERIVED_LAYERS.len();
    for (index, layer) in DERIVED_LAYERS.into_iter().enumerate() {
        layers.insert(
            layer.to_string(),
            json!(base + usize::from(index < remainder)),
        );
    }
    Value::Object(layers)
}

/// The answer request body.
///
/// The hosted route's schema is strict (an unknown key, or a `null`
/// `answer_instructions`, is a 400), so the hosted body omits instructions
/// when there are none. Direct keeps `answer_instructions: null`.
pub(super) fn answer_body(
    wire: CortexWire,
    scope: &str,
    question: &str,
    pack_id: &str,
    instructions: Option<&str>,
) -> Value {
    let mut body = json!({
        "scope": scope,
        "question": question,
        "use_pack_id": pack_id,
        "cite_sources": true,
        "include_context": true,
    });
    match (wire, instructions) {
        (_, Some(text)) => body["answer_instructions"] = json!(text),
        (CortexWire::Direct, None) => body["answer_instructions"] = Value::Null,
        (CortexWire::TinyHumans, None) => {}
    }
    body
}

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn recall_answer(&self, req: RecallRequest) -> Result<RecallAnswer> {
        req.validate()?;
        let kinds = admitted(&req.filter);
        let (scope, descend) = match kinds.as_slice() {
            [single] => (scope_of(*single), false),
            _ => (ROOT_SCOPE, true),
        };
        let mut body = recall_body(scope, &req.question, 0, &req.filter);
        body["budgets"]["per_layer_limits"] = pack_budgets(req.limit);
        if descend {
            body["view"] = json!("descend");
        }
        let pack = self.log.recall(&body).await?;
        let pack_id = pack
            .get("pack_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Engine("CortexDB recall omitted pack_id".to_string()))?;
        let answered = self
            .log
            .answer(&answer_body(
                self.wire(),
                scope,
                &req.question,
                pack_id,
                req.instructions.as_deref(),
            ))
            .await?;
        let answer = answered
            .get("answer")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Engine("CortexDB omitted the answer text".to_string()))?
            .to_string();
        let citations = ranked(&pack, None, &req.filter)
            .into_iter()
            .take(req.limit)
            .map(|envelope| Citation {
                id: ItemId::new(envelope.id),
                kind: envelope.kind,
                snippet: envelope.text,
                meta: envelope.meta,
                score: None,
            })
            .collect();
        Ok(RecallAnswer {
            answer,
            citations,
            model: answered
                .pointer("/diagnostics/answer_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
