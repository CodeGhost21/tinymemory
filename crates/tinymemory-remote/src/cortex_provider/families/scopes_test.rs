//! Bookkeeping scopes stay out of every namespace.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::cortex::CortexDialect;

#[test]
fn a_bookkeeping_scope_is_reachable_from_no_namespace() {
    let details = document_details("tm:notes");
    for scope in [GOALS, DOCUMENT_DETAILS, details.as_str()] {
        assert_eq!(CortexDialect::namespace_of(scope), None, "{scope}");
    }
    for namespace in ["goals", "tmi:goals", "documents", "tmi", "tmi/goals"] {
        let scope = CortexDialect::scope_of(namespace).expect("maps");
        assert!(
            scope.split('/').all(|segment| !segment.starts_with("tmi:")),
            "{namespace} -> {scope}"
        );
    }
}
