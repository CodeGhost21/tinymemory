//! Descriptor mode checks and health classification.

use super::*;

fn descriptor(modes: Vec<FetchMode>) -> EngineDescriptor {
    EngineDescriptor {
        id: "test",
        label: "Test",
        description: "A test engine.",
        hosted: false,
        needs_endpoint: false,
        needs_key: false,
        default_endpoint: None,
        fetch_modes: modes,
        consolidation: Consolidation::None,
    }
}

#[test]
fn an_undeclared_mode_is_unsupported() {
    let descriptor = descriptor(vec![FetchMode::Hybrid]);
    assert!(descriptor.supports(FetchMode::Hybrid));
    assert!(descriptor.ensure_mode(FetchMode::Hybrid).is_ok());
    let error = descriptor
        .ensure_mode(FetchMode::Vector)
        .expect_err("vector is undeclared");
    assert_eq!(
        error,
        Error::Unsupported("engine `test` does not offer vector fetch".to_string())
    );
}

#[test]
fn only_down_is_not_serving() {
    assert!(EngineHealth::Ok.is_serving());
    assert!(EngineHealth::Degraded("slow".into()).is_serving());
    assert!(!EngineHealth::Down("gone".into()).is_serving());
    assert_eq!(
        serde_json::to_value(EngineHealth::Down("gone".into())).expect("json"),
        serde_json::json!({ "state": "down", "reason": "gone" })
    );
}
