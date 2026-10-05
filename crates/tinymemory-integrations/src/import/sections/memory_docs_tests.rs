//! Tests for `memory_docs` namespace restoration and classification.

use super::*;

#[test]
fn restores_sanitised_section_prefixes() {
    assert_eq!(restore_namespace("learning_style"), "learning:style");
    assert_eq!(restore_namespace("document_abc_def"), "document:abc_def");
    assert_eq!(restore_namespace("source:gh"), "source:gh");
    assert_eq!(restore_namespace("user_notes"), "user_notes");
    assert_eq!(restore_namespace("global"), "global");
}

#[test]
fn classifies_logical_namespaces() {
    assert_eq!(
        classify("learning:style"),
        RowClass::Learning(Some("style".into()))
    );
    assert_eq!(classify("learning"), RowClass::Learning(None));
    assert_eq!(classify("learning:"), RowClass::Learning(None));
    assert_eq!(classify("global"), RowClass::Global);
    assert_eq!(classify("event:chat"), RowClass::Event);
    assert_eq!(classify("event"), RowClass::Event);
    assert_eq!(classify("document:x"), RowClass::Document);
    assert_eq!(classify("source:x"), RowClass::Document);
    assert_eq!(classify("eventually"), RowClass::Document);
    assert_eq!(classify("user_notes"), RowClass::Document);
}

#[test]
fn decodes_taint_failing_closed() {
    assert!(!is_external(Some("internal")));
    assert!(!is_external(Some(" INTERNAL ")));
    assert!(is_external(Some("external_sync")));
    assert!(is_external(Some("sideloaded")));
    assert!(is_external(Some("")));
    assert!(is_external(None));
}
