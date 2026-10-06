//! Tests for the pack budgets and the answer body per wire.

use super::*;

#[test]
fn the_hosted_answer_body_omits_absent_instructions_and_direct_sends_null() {
    let hosted = answer_body(CortexWire::TinyHumans, "s", "q", "p", None);
    assert!(hosted.get("answer_instructions").is_none());
    assert_eq!(hosted["use_pack_id"], "p");
    let direct = answer_body(CortexWire::Direct, "s", "q", "p", None);
    assert!(direct["answer_instructions"].is_null());
    assert!(direct.get("answer_instructions").is_some());
    let with = answer_body(CortexWire::TinyHumans, "s", "q", "p", Some("be brief"));
    assert_eq!(with["answer_instructions"], "be brief");
}

#[test]
fn pack_budgets_cover_the_limit_across_layers() {
    let budgets = pack_budgets(6);
    assert_eq!(budgets["events"], 12);
    let derived: u64 = DERIVED_LAYERS
        .iter()
        .map(|layer| budgets[*layer].as_u64().unwrap())
        .sum();
    assert_eq!(derived, 6);
}

#[test]
fn an_answer_pack_funds_its_events_before_the_derived_layers() {
    assert_eq!(
        pack_layers(),
        json!(["events", "facts", "beliefs", "episodes", "understanding"])
    );
}
