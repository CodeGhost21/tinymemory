//! Attribution: who said or did what an event records.
//!
//! CortexDB 0.10.5 takes an `observed_actor` (`{id, type}`, defaulting to the
//! caller) and a `subject` (defaulting to the observed actor) on every
//! write. Naming an actor other than the caller needs the
//! `scope.write.on_behalf_of` capability, and a subject other than the actor
//! `scope.write.about_other`; without them CortexDB refuses the write
//! (`403 POLICY_DENIED`), a bulk write whole.
//!
//! With attribution on (`EngineSettings::observed_actor`, direct wire only):
//!
//! - an assistant turn is observed from its agent (`agent:<agent_id>`);
//! - an item naming a [`tinymemory_api::ObservedActor`] (the sender of a
//!   synced email) is observed from that person;
//! - anything else is the owner's own and carries neither field;
//! - an attributed event's subject is the memory's owner (the v3 root's
//!   owner, else the caller `whoami` reports).
//!
//! A write CortexDB refuses for it (any 401/403, or 400/413/422) is sent
//! again without the fields, so turning attribution on never loses a write.
//! A refusal the plain retry then gets past as a permission fault turns
//! attribution off for the engine's life: the credential lacks the
//! capabilities, and every write would pay a second request. A validation
//! refusal drops it for that write only.
//!
//! Off, nothing on the wire changes: no field is sent, and an item's
//! `observed_actor` is cleared before its events are laid out, so their
//! text and idempotency keys are what they were before the field existed.
//! A phone number is never sent or stored through it: an actor whose id
//! looks like one is dropped, and so is a name holding one.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use tinymemory_api::{MemoryMeta, Role};

use crate::cortex::envelope::Envelope;
use crate::cortex::transport::body_idempotency_key;

/// Whether attribution is on, and whether CortexDB refused it.
#[derive(Debug, Default)]
pub(crate) struct Attribution {
    enabled: bool,
    refused: AtomicBool,
}

impl Attribution {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            refused: AtomicBool::new(false),
        }
    }

    /// On, and not refused yet.
    pub(crate) fn active(&self) -> bool {
        self.enabled && !self.refused.load(Ordering::SeqCst)
    }

    /// Turns attribution off after CortexDB refused it as a permission.
    pub(crate) fn refuse(&self, reason: &str) {
        if !self.refused.swap(true, Ordering::SeqCst) {
            log::warn!(
                "[cortex] attribution refused, writing without observed_actor from now on: {reason}"
            );
        }
    }
}

/// Keeps `meta.observed_actor` only when attributing, and only without a
/// phone number in it.
pub(crate) fn screen(meta: &mut MemoryMeta, attributing: bool) {
    if !attributing {
        meta.observed_actor = None;
        return;
    }
    if let Some(actor) = meta.observed_actor.as_mut() {
        actor.name = actor.name.take().filter(|name| !holds_phone(name));
    }
    if meta
        .observed_actor
        .as_ref()
        .is_some_and(|actor| actor_id(&actor.id).is_none())
    {
        meta.observed_actor = None;
    }
}

/// Who `envelope`'s event is observed from, when someone other than the
/// memory's owner: an assistant turn's agent, or the item's named actor.
pub(crate) fn actor_of(envelope: &Envelope) -> Option<String> {
    match &envelope.turn {
        Some(turn) if turn.role == Role::Assistant => envelope
            .meta
            .agent_id
            .as_deref()
            .and_then(|agent| actor_id(&format!("agent:{}", agent.trim()))),
        Some(_) => None,
        None => envelope
            .meta
            .observed_actor
            .as_ref()
            .and_then(|actor| actor_id(&actor.id)),
    }
}

/// Adds `observed_actor` and `subject` to a built request and keys it anew.
pub(crate) fn attribute(request: &mut Value, actor: &str, subject: &str) {
    request["observed_actor"] = typed(actor);
    request["subject"] = typed(subject);
    rekey(request);
}

/// Removes what [`attribute`] added, restoring the request's own key.
pub(crate) fn strip(request: &mut Value) {
    if let Some(object) = request.as_object_mut() {
        object.remove("observed_actor");
        object.remove("subject");
    }
    rekey(request);
}

fn rekey(request: &mut Value) {
    request["idempotency_key"] = json!(body_idempotency_key(request));
}

/// `{id, type}` for a `type:id` actor.
fn typed(id: &str) -> Value {
    let kind = id.split_once(':').map_or(id, |(kind, _)| kind);
    json!({ "id": id, "type": kind })
}

/// `id` as an actor id: `type:id`, both parts present, no whitespace, and
/// not a phone number.
fn actor_id(id: &str) -> Option<String> {
    let id = id.trim();
    let (kind, rest) = id.split_once(':')?;
    let clean = |part: &str| !part.is_empty() && !part.chars().any(char::is_whitespace);
    (clean(kind) && clean(rest) && !looks_like_phone(rest)).then(|| id.to_string())
}

/// Only phone characters, with enough digits to dial.
fn looks_like_phone(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_digit() || "+-(). ".contains(c))
        && holds_phone(text)
}

/// Seven or more digits: enough to be a phone number. A display name has
/// no business holding that many.
fn holds_phone(text: &str) -> bool {
    text.chars().filter(char::is_ascii_digit).count() >= 7
}

#[cfg(test)]
#[path = "attribution_tests.rs"]
mod tests;
