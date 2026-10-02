//! Tests for resolving full chunk bodies.

use super::*;

#[test]
fn reads_a_body_inside_the_content_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("a")).expect("mkdir");
    std::fs::write(dir.path().join("a/b.md"), "full body").expect("write");
    assert_eq!(
        full_body(dir.path(), "a/b.md").expect("reads").as_deref(),
        Some("full body")
    );
}

#[test]
fn a_missing_body_falls_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(full_body(dir.path(), "nope.md").expect("reads"), None);
}

#[test]
fn refuses_paths_that_escape_the_content_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("secret"), "x").expect("write");
    let inner = dir.path().join("content");
    std::fs::create_dir_all(&inner).expect("mkdir");
    assert_eq!(full_body(&inner, "../secret").expect("reads"), None);
    let absolute = dir.path().join("secret");
    assert_eq!(
        full_body(&inner, &absolute.display().to_string()).expect("reads"),
        None
    );
    assert_eq!(full_body(&inner, "").expect("reads"), None);
}

#[test]
fn an_unreadable_body_is_an_io_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("dir.md")).expect("mkdir");
    let err = full_body(dir.path(), "dir.md").expect_err("a directory is not a body");
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
}
