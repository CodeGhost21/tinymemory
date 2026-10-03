//! Tests for the descriptors and route table.

use super::*;

#[test]
fn the_direct_descriptor_needs_a_key_and_defaults_its_endpoint() {
    let d = cortexdb_descriptor();
    assert_eq!(d.id, "cortexdb");
    assert!(!d.hosted);
    assert!(!d.needs_endpoint);
    assert!(d.needs_key);
    assert_eq!(d.default_endpoint, Some("https://api-v1.cortexdb.ai"));
    assert_eq!(d.fetch_modes, vec![FetchMode::Hybrid]);
}

#[test]
fn the_hosted_descriptor_is_hosted_and_hybrid_only() {
    let d = tinyhumans_descriptor();
    assert_eq!(d.id, "tinyhumans");
    assert!(d.hosted);
    assert!(!d.needs_endpoint);
    assert!(d.needs_key);
    assert_eq!(d.default_endpoint, Some("https://api.tinyhumans.ai"));
    assert!(d.supports(FetchMode::Hybrid));
    assert!(!d.supports(FetchMode::Keyword));
    assert!(!d.supports(FetchMode::Vector));
}

#[test]
fn every_hosted_route_is_under_memory_and_every_direct_one_under_v1() {
    let routes = [
        Route::Experience,
        Route::Bulk,
        Route::Events,
        Route::Recall,
        Route::Forget,
        Route::Answer,
        Route::Health,
        Route::Scopes,
    ];
    for route in routes {
        assert!(CortexWire::Direct.path(route).starts_with("v1/"));
        assert!(CortexWire::TinyHumans.path(route).starts_with("memory/"));
    }
    assert_eq!(CortexWire::TinyHumans.descriptor().id, TINYHUMANS_ENGINE_ID);
    assert_eq!(CortexWire::Direct.descriptor().id, CORTEXDB_ENGINE_ID);
}
