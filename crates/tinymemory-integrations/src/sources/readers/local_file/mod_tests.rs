//! Tests for the path-containment guard the local readers share.

use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn ensure_within_base_accepts_contained_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("ok.md"), "hi").unwrap();
    let resolved = ensure_within_base(tmp.path(), &tmp.path().join("ok.md")).unwrap();
    assert!(resolved.ends_with("ok.md"));
}

#[test]
fn ensure_within_base_rejects_escape() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("ok.md"), "hi").unwrap();
    // Build a target that escapes the base via `..`.
    let escaping = tmp.path().join("../../etc/hosts");
    let result = ensure_within_base(tmp.path(), &escaping);
    assert!(result.is_err());
}
