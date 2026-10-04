//! Error rendering and classification.

use super::*;

#[test]
fn every_variant_renders_its_class_and_message() {
    let cases = [
        (Error::Unsupported("x".into()), "unsupported: x"),
        (Error::InvalidRequest("x".into()), "invalid request: x"),
        (Error::Unauthorized("x".into()), "unauthorized: x"),
        (Error::NotFound("x".into()), "not found: x"),
        (Error::Conflict("x".into()), "conflict: x"),
        (Error::Unavailable("x".into()), "unavailable: x"),
        (Error::Engine("x".into()), "engine error: x"),
        (Error::Config("x".into()), "configuration error: x"),
    ];
    for (error, rendered) in cases {
        assert_eq!(error.to_string(), rendered);
    }
}

#[test]
fn only_unavailable_is_transient() {
    assert!(Error::Unavailable("busy".into()).is_transient());
    assert!(!Error::Engine("broken".into()).is_transient());
    assert!(!Error::Unauthorized("expired".into()).is_transient());
}
