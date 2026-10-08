//! The attribution rules: whom an event is observed from, and what is sent.

use super::*;
use tinymemory_api::{ObservedActor, StoreItem, Turn};

use crate::cortex::envelope::ScopeLayout;

fn sender(id: &str, name: Option<&str>) -> MemoryMeta {
    MemoryMeta {
        observed_actor: Some(ObservedActor {
            id: id.into(),
            name: name.map(str::to_string),
        }),
        ..MemoryMeta::default()
    }
}

fn actors(item: &StoreItem) -> Vec<Option<String>> {
    Envelope::for_item(item, &item.fingerprint())
        .unwrap()
        .iter()
        .map(actor_of)
        .collect()
}

#[test]
fn an_assistant_turn_is_its_agents_and_a_user_turn_the_owners() {
    let meta = MemoryMeta {
        agent_id: Some("orchestrator".into()),
        ..MemoryMeta::default()
    };
    let chat = StoreItem::Conversation {
        turns: vec![
            Turn::new(Role::User, "hello"),
            Turn::new(Role::Assistant, "hi"),
            Turn::new(Role::Tool, "result"),
        ],
        meta,
    };
    assert_eq!(
        actors(&chat),
        [None, Some("agent:orchestrator".to_string()), None]
    );
    let anonymous = StoreItem::Conversation {
        turns: vec![Turn::new(Role::Assistant, "hi")],
        meta: MemoryMeta::default(),
    };
    assert_eq!(actors(&anonymous), [None], "no agent id, no actor");
}

#[test]
fn a_document_is_its_named_senders() {
    let email = StoreItem::document("lunch?", sender("user:priya@acme.com", Some("Priya")));
    assert_eq!(actors(&email), [Some("user:priya@acme.com".to_string())]);
    let plain = StoreItem::document("note", MemoryMeta::default());
    assert_eq!(actors(&plain), [None]);
}

#[test]
fn off_clears_the_named_actor_and_on_keeps_it_without_phone_numbers() {
    let mut meta = sender("user:priya@acme.com", Some("Priya"));
    screen(&mut meta, false);
    assert_eq!(meta.observed_actor, None, "off: never laid out");

    let mut meta = sender("user:priya@acme.com", Some("Priya"));
    screen(&mut meta, true);
    assert_eq!(meta, sender("user:priya@acme.com", Some("Priya")));

    for phone in [
        "user:+1 (555) 123-4567",
        "user:15551234567",
        "nocolon",
        "user:",
    ] {
        let mut meta = sender(phone, None);
        screen(&mut meta, true);
        assert_eq!(meta.observed_actor, None, "{phone}");
    }

    let mut meta = sender("user:priya@acme.com", Some("Priya +44 7700 900123"));
    screen(&mut meta, true);
    assert_eq!(meta, sender("user:priya@acme.com", None), "the name goes");
}

#[test]
fn stripping_restores_the_plain_request_and_its_key() {
    let item = StoreItem::document("lunch?", MemoryMeta::default());
    let envelope = &Envelope::for_item(&item, &item.fingerprint()).unwrap()[0];
    let plain = envelope.request(&envelope.encode_checked().unwrap(), &ScopeLayout::default());

    let mut request = plain.clone();
    attribute(&mut request, "user:priya@acme.com", "user:42");
    assert_eq!(
        request["observed_actor"],
        json!({"id": "user:priya@acme.com", "type": "user"})
    );
    assert_eq!(request["subject"], json!({"id": "user:42", "type": "user"}));
    assert_ne!(
        request["idempotency_key"], plain["idempotency_key"],
        "an attributed body is its own write"
    );

    strip(&mut request);
    assert_eq!(request, plain);
}

#[test]
fn a_refusal_turns_attribution_off() {
    assert!(!Attribution::default().active(), "off by default");
    let attribution = Attribution::new(true);
    assert!(attribution.active());
    attribution.refuse("403 POLICY_DENIED");
    assert!(!attribution.active());
}
