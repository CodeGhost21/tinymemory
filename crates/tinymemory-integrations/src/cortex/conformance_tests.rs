//! The shared conformance suite, run against both wires through the doubles.

use crate::cortex::testing::{direct_double, direct_engine, hosted_double, hosted_engine};

#[tokio::test]
async fn the_direct_wire_upholds_the_contract() {
    let (endpoint, _state) = direct_double().await;
    tinymemory_api::conformance::run(&direct_engine(&endpoint))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_tinyhumans_wire_upholds_the_contract() {
    let (endpoint, _state) = hosted_double().await;
    tinymemory_api::conformance::run(&hosted_engine(&endpoint))
        .await
        .unwrap();
}

#[tokio::test]
async fn both_wires_uphold_the_contract_in_layout_v3() {
    let (endpoint, _state) = direct_double().await;
    let direct = direct_engine(&endpoint)
        .with_scope_root("user:42", Some("user:42"))
        .unwrap();
    tinymemory_api::conformance::run(&direct).await.unwrap();
    let (endpoint, _state) = hosted_double().await;
    let hosted = hosted_engine(&endpoint)
        .with_scope_root("user:6512ab0f", None)
        .unwrap();
    tinymemory_api::conformance::run(&hosted).await.unwrap();
}
