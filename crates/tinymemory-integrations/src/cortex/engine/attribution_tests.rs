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
fn a_user_turn_is_its_named_senders_and_other_turns_keep_their_rule() {
    let meta = MemoryMeta {
        agent_id: Some("orchestrator".into()),
        ..sender("user:+15551234567", Some("Priya"))
    };
    let chat = StoreItem::Conversation {
        turns: vec![
            Turn::new(Role::User, "is my order shipped"),
            Turn::new(Role::Assistant, "It ships today."),
            Turn::new(Role::Tool, "result"),
            Turn::new(Role::System, "note"),
        ],
        meta,
    };
    assert_eq!(
        actors(&chat),
        [
            Some("user:+15551234567".to_string()),
            Some("agent:orchestrator".to_string()),
            None,
            None,
        ]
    );
    let malformed = StoreItem::Conversation {
        turns: vec![Turn::new(Role::User, "hi")],
        meta: sender("+15551234567", None),
    };
    assert_eq!(actors(&malformed), [None], "an actor id must be type:id");
}

#[test]
fn a_document_is_its_named_senders() {
    let email = StoreItem::document("lunch?", sender("user:priya@acme.com", Some("Priya")));
    assert_eq!(actors(&email), [Some("user:priya@acme.com".to_string())]);
    let plain = StoreItem::document("note", MemoryMeta::default());
    assert_eq!(actors(&plain), [None]);
}

#[test]
fn off_clears_the_named_actor_and_on_keeps_any_well_formed_one() {
    let mut meta = sender("user:priya@acme.com", Some("Priya"));
    screen(&mut meta, false);
    assert_eq!(meta.observed_actor, None, "off: never laid out");

    for kept in [
        sender("user:priya@acme.com", Some("Priya")),
        sender("user:+15551234567", Some("Mum +44 7700 900123")),
    ] {
        let mut meta = kept.clone();
        screen(&mut meta, true);
        assert_eq!(meta, kept, "a phone number is kept as it is");
    }

    for malformed in ["nocolon", "user:", ":priya", "user:+1 555 123 4567"] {
        let mut meta = sender(malformed, None);
        screen(&mut meta, true);
        assert_eq!(meta.observed_actor, None, "{malformed}");
    }
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
