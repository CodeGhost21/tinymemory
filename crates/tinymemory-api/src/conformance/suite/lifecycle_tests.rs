//! How a consolidation's answer is held to the descriptor's promise.

use super::*;
use crate::ConsolidateReceipt;

fn receipt(status: ConsolidateStatus) -> ConsolidateReceipt {
    ConsolidateReceipt {
        status,
        jobs: Vec::new(),
        scopes: 1,
        built: None,
    }
}

#[test]
fn an_automatic_engine_answers_an_explicit_build_like_an_on_demand_one() {
    for status in [ConsolidateStatus::Started, ConsolidateStatus::Completed] {
        answers_as_promised(Consolidation::Automatic, Ok(receipt(status))).unwrap();
        answers_as_promised(Consolidation::OnDemand, Ok(receipt(status))).unwrap();
    }
}

#[test]
fn an_automatic_engine_may_not_merely_acknowledge_an_explicit_build() {
    let outcome = answers_as_promised(
        Consolidation::Automatic,
        Ok(ConsolidateReceipt::scheduled()),
    );
    assert!(matches!(outcome, Err(Error::Check { .. })), "{outcome:?}");
}
