//! Attribution through `store_many`: off changes nothing, on names the
//! actor, and a refusal never loses the write.

use super::*;
use serde_json::Value;
use tinymemory_api::{
    ListRequest, MemoryEngine, MemoryMeta, MetaFilter, ObservedActor, Role, StoreItem, Turn,
};

use crate::cortex::testing::{Shared, direct_double, direct_engine, hosted_double, hosted_engine};

fn chat() -> StoreItem {
    StoreItem::Conversation {
        turns: vec![
            Turn::new(Role::User, "lunch tomorrow?"),
            Turn::new(Role::Assistant, "Booked a table at noon."),
        ],
        meta: MemoryMeta {
            agent_id: Some("orchestrator".into()),
            ..MemoryMeta::default()
        },
    }
}

fn email(body: &str, sender: bool) -> StoreItem {
    let mut meta = MemoryMeta::default();
    if sender {
        meta.observed_actor = Some(ObservedActor {
            id: "user:priya@acme.com".into(),
            name: Some("Priya".into()),
        });
    }
    StoreItem::document(body, meta)
}

/// Every event body written, a bulk write's items one by one.
fn events(state: &Shared) -> Vec<Value> {
    let seen = state.seen.lock().unwrap();
    seen.writes
        .iter()
        .flat_map(|write| match write["items"].as_array() {
            Some(items) => items.clone(),
            None => vec![write.clone()],
        })
        .collect()
}

fn actor(event: &Value) -> Option<&str> {
    event["observed_actor"]["id"].as_str()
}

async fn on_direct() -> (CortexEngine, Shared) {
    let (endpoint, state) = direct_double().await;
    let engine = direct_engine(&endpoint)
        .with_scope_root("user:42", Some("user:42"))
        .unwrap()
        .with_observed_actor(true);
    (engine, state)
}

#[tokio::test]
async fn off_writes_the_same_bytes_as_before_the_field_existed() {
    let mut written = Vec::new();
    for (sender, on) in [(false, false), (true, false), (true, true)] {
        let (endpoint, state) = direct_double().await;
        let direct = direct_engine(&endpoint)
            .with_scope_root("user:42", Some("user:42"))
            .unwrap();
        // Off by default; `on` here is the hosted engine, which never
        // attributes.
        let (hosted_endpoint, hosted_state) = hosted_double().await;
        let hosted = hosted_engine(&hosted_endpoint).with_observed_actor(on);
        let items = vec![chat(), email("see you at noon", sender)];
        direct.store_many(items.clone()).await.unwrap();
        hosted.store_many(items).await.unwrap();
        written.push((events(&state), events(&hosted_state)));
    }
    assert!(
        written.iter().all(|w| w.0 == written[0].0),
        "direct, off: same bodies and idempotency keys with or without a sender"
    );
    assert!(
        written.iter().all(|w| w.1 == written[0].1),
        "hosted: inert even when on"
    );
    assert!(written[0].0.iter().all(|event| actor(event).is_none()));
}

#[tokio::test]
async fn on_names_the_agent_and_the_sender_with_the_owner_as_subject() {
    let (engine, state) = on_direct().await;
    engine
        .store_many(vec![
            chat(),
            email("see you at noon", true),
            email("memo", false),
        ])
        .await
        .unwrap();
    let events = events(&state);
    let actors: Vec<Option<&str>> = events.iter().map(actor).collect();
    assert_eq!(
        actors,
        [
            None,
            Some("agent:orchestrator"),
            Some("user:priya@acme.com"),
            None
        ],
        "the user's own turn and an unattributed document carry neither field"
    );
    for event in events.iter().filter(|event| actor(event).is_some()) {
        assert_eq!(event["subject"]["id"], "user:42");
        assert_eq!(event["subject"]["type"], "user");
    }
    assert_eq!(events[1]["observed_actor"]["type"], "agent");
    assert!(events[0].get("subject").is_none());

    let listed = engine
        .list(ListRequest::new(MetaFilter::default(), 10))
        .await
        .unwrap();
    let sender = listed
        .items
        .iter()
        .find_map(|item| item.meta.observed_actor.clone());
    assert_eq!(
        sender.and_then(|actor| actor.name),
        Some("Priya".to_string()),
        "the sender's name is kept with the item"
    );
}

#[tokio::test]
async fn a_permission_refusal_writes_plainly_and_turns_attribution_off() {
    let (engine, state) = on_direct().await;
    *state.refuse_attribution.lock().unwrap() = Some((403, "POLICY_DENIED"));

    let receipts = engine
        .store_many(vec![email("see you at noon", true)])
        .await
        .unwrap();
    assert_eq!(receipts.len(), 1, "the write is not lost");
    let first: Vec<Option<String>> = events(&state)
        .iter()
        .map(|e| actor(e).map(str::to_string))
        .collect();
    assert_eq!(
        first,
        [Some("user:priya@acme.com".to_string()), None],
        "refused, then written without"
    );

    engine
        .store_many(vec![email("and the week after", true)])
        .await
        .unwrap();
    let events = events(&state);
    assert_eq!(events.len(), 3, "no second refused attempt");
    assert!(actor(&events[2]).is_none(), "attribution stays off");
}

#[tokio::test]
async fn a_validation_refusal_writes_plainly_and_keeps_trying() {
    let (engine, state) = on_direct().await;
    *state.refuse_attribution.lock().unwrap() = Some((422, "VALIDATION_ERROR"));

    for body in ["see you at noon", "and the week after"] {
        engine.store_many(vec![email(body, true)]).await.unwrap();
    }
    let actors: Vec<bool> = events(&state).iter().map(|e| actor(e).is_some()).collect();
    assert_eq!(actors, [true, false, true, false]);
}

#[tokio::test]
async fn a_refused_conversation_is_written_whole_without_attribution() {
    let (engine, state) = on_direct().await;
    *state.refuse_attribution.lock().unwrap() = Some((403, "POLICY_DENIED"));
    engine.store_many(vec![chat()]).await.unwrap();
    let events = events(&state);
    assert_eq!(events.len(), 4, "a refused bulk of two, then both again");
    assert!(events[2..].iter().all(|e| actor(e).is_none()));
    assert_eq!(state.log.lock().unwrap().events.len(), 2, "each turn once");
}

#[tokio::test]
async fn without_a_root_owner_the_subject_is_the_whoami_caller() {
    let (endpoint, state) = direct_double().await;
    *state.whoami_caller.lock().unwrap() = Some("user:local".into());
    let engine = direct_engine(&endpoint).with_observed_actor(true);
    engine.store_many(vec![chat()]).await.unwrap();
    let events = events(&state);
    assert_eq!(actor(&events[1]), Some("agent:orchestrator"));
    assert_eq!(events[1]["subject"]["id"], "user:local");
}

#[tokio::test]
async fn nothing_is_attributed_without_a_well_formed_subject() {
    for (owner, caller) in [(None, None), (None, Some("local")), (Some("owner"), None)] {
        let (endpoint, state) = direct_double().await;
        *state.whoami_caller.lock().unwrap() = caller.map(str::to_string);
        let mut engine = direct_engine(&endpoint);
        if let Some(owner) = owner {
            engine = engine.with_scope_root("user:42", Some(owner)).unwrap();
        }
        let engine = engine.with_observed_actor(true);
        engine
            .store_many(vec![chat(), email("see you at noon", true)])
            .await
            .unwrap();
        assert!(
            events(&state).iter().all(
                |event| event.get("observed_actor").is_none() && event.get("subject").is_none()
            ),
            "owner {owner:?}, caller {caller:?}: written plainly"
        );
    }
}
