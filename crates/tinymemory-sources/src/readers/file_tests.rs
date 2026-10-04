//! Tests for the single-file reader.

use super::*;

use std::fs;
use tempfile::TempDir;

fn file_source(path: &str) -> MemorySourceEntry {
    MemorySourceEntry {
        path: Some(path.to_string()),
        ..MemorySourceEntry::new("src_file", SourceKind::File, "One file")
    }
}

#[tokio::test]
async fn lists_exactly_the_configured_file() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("plan.md"), "# Plan").unwrap();
    let items = FileReader
        .list_items(&file_source("plan.md"), workspace.path())
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "plan.md");
    assert!(items[0].updated_at_ms.is_some());
}

#[tokio::test]
async fn reads_the_listed_file_and_refuses_any_other_id() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("page.html"), "<p>hi</p>").unwrap();
    let source = file_source("page.html");

    let content = FileReader
        .read_item(&source, "page.html", workspace.path())
        .await
        .unwrap();
    assert_eq!(content.body, "<p>hi</p>");
    assert_eq!(content.content_type, ContentType::Html);

    let error = FileReader
        .read_item(&source, "other.md", workspace.path())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "got {error:?}");
}

#[tokio::test]
async fn a_missing_path_or_file_is_reported() {
    let workspace = TempDir::new().unwrap();
    let mut source = file_source("x");
    source.path = None;
    let error = FileReader
        .list_items(&source, workspace.path())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("file source requires a path"));

    let error = FileReader
        .list_items(&file_source("missing.md"), workspace.path())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)), "got {error:?}");
}

#[test]
fn read_path_refuses_directories_and_oversized_files() {
    let dir = TempDir::new().unwrap();
    let error = FileReader::read_path(dir.path()).unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "got {error:?}");

    let huge = dir.path().join("huge.txt");
    let file = fs::File::create(&huge).unwrap();
    file.set_len(crate::FOLDER_FILE_SIZE_CAP_BYTES + 1).unwrap();
    drop(file);
    let error = FileReader::read_path(&huge).unwrap_err();
    assert!(matches!(error, Error::TooLarge(_)), "got {error:?}");
}

#[test]
fn read_path_returns_the_canonical_path_bytes_and_mtime() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.rs");
    fs::write(&path, "fn a() {}").unwrap();
    let file = FileReader::read_path(&path).unwrap();
    assert_eq!(file.id, "a.rs");
    assert_eq!(file.bytes, b"fn a() {}");
    assert_eq!(file.path, fs::canonicalize(&path).unwrap());
    assert!(file.modified.is_some());
    assert_eq!(file.text().unwrap(), "fn a() {}");
}
