//! Tests for the `StoreItem` emission layer: the metadata each source kind
//! fills, conversion to markdown, and the collect pass.

use super::*;

use std::fs;

use crate::documents::ConverterChain;
use async_trait::async_trait;
use tempfile::TempDir;
use tinymemory_api::{ItemKind, Role, SourceKind as Api, Turn};

use crate::sources::readers::conversation::ConversationReader;
use crate::sources::readers::file::FileReader;
use crate::sources::readers::folder::FolderReader;
use crate::sources::types::SourceItem;

fn entry(kind: SourceKind) -> MemorySourceEntry {
    MemorySourceEntry::new("src_test", kind, "Test")
}

fn content(
    id: &str,
    body: &str,
    content_type: ContentType,
    metadata: serde_json::Value,
) -> SourceContent {
    SourceContent {
        id: id.to_string(),
        title: format!("title of {id}"),
        body: body.to_string(),
        content_type,
        metadata,
    }
}

fn document_parts(item: &StoreItem) -> (&Option<String>, &str, &Option<String>, &MemoryMeta) {
    match item {
        StoreItem::Document {
            title,
            body: DocumentBody::Text(text),
            mime,
            meta,
        } => (title, text.as_str(), mime, meta),
        other => panic!("expected a text document, got {other:?}"),
    }
}

#[test]
fn base_meta_names_the_source_by_contract_kind_and_entry_id() {
    for (kind, api) in [
        (SourceKind::Folder, Api::Folder),
        (SourceKind::File, Api::File),
        (SourceKind::Conversation, Api::Conversation),
    ] {
        let meta = base_meta(&entry(kind));
        assert_eq!(meta.source.kind, api);
        assert_eq!(meta.source.id.as_deref(), Some("src_test"));
    }
}

#[test]
fn reader_content_for_local_kinds_fills_what_it_can() {
    let mut folder = entry(SourceKind::Folder);
    folder.path = Some("/notes".into());
    let item = content_item(
        &folder,
        content(
            "src/lib.rs",
            "pub fn f() {}",
            ContentType::Plaintext,
            serde_json::json!({}),
        ),
        None,
    )
    .unwrap();
    assert_eq!(item.meta().file_path.as_deref(), Some("/notes/src/lib.rs"));
    assert_eq!(item.meta().folder.as_deref(), Some("/notes/src"));
    assert_eq!(item.meta().language.as_deref(), Some("rust"));

    let file = content_item(
        &entry(SourceKind::File),
        content(
            "a.py",
            "x = 1",
            ContentType::Plaintext,
            serde_json::json!({ "path": "/w/a.py" }),
        ),
        None,
    )
    .unwrap();
    assert_eq!(file.meta().file_path.as_deref(), Some("/w/a.py"));
    assert_eq!(file.meta().language.as_deref(), Some("python"));

    let conversation = content_item(
        &entry(SourceKind::Conversation),
        content(
            "t1",
            "**user**: hi",
            ContentType::Markdown,
            serde_json::json!({}),
        ),
        None,
    )
    .unwrap();
    assert_eq!(conversation.meta().thread_id.as_deref(), Some("t1"));
}

#[test]
fn content_that_converts_to_nothing_is_refused() {
    let error = content_item(
        &entry(SourceKind::Folder),
        content(
            "u",
            "<script>x()</script>",
            ContentType::Html,
            serde_json::json!({}),
        ),
        None,
    )
    .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "got {error:?}");
}

#[test]
fn a_thread_becomes_a_conversation_with_range_and_last_turn_time() {
    let at = Utc.timestamp_opt(1_700_000_100, 0).single();
    let thread = Thread {
        id: "thread_1".into(),
        title: Some("Chat".into()),
        turns: vec![
            Turn::new(Role::User, "hi"),
            Turn {
                at,
                ..Turn::new(Role::Assistant, "hello")
            },
        ],
        modified: Utc.timestamp_opt(1, 0).single(),
    };
    let item = conversation_item(thread, MemoryMeta::default()).unwrap();
    assert_eq!(item.kind(), ItemKind::Conversation);
    item.validate().unwrap();
    let meta = item.meta();
    assert_eq!(meta.thread_id.as_deref(), Some("thread_1"));
    assert_eq!(meta.turns, Some(TurnRange { first: 0, last: 1 }));
    assert_eq!(meta.observed_at, at);
}

#[test]
fn a_thread_without_turns_is_refused_and_untimed_turns_fall_back_to_mtime() {
    let empty = Thread {
        id: "t".into(),
        title: None,
        turns: Vec::new(),
        modified: None,
    };
    assert!(matches!(
        conversation_item(empty, MemoryMeta::default()),
        Err(Error::Invalid(_))
    ));

    let modified = Utc.timestamp_opt(5, 0).single();
    let untimed = Thread {
        id: "t".into(),
        title: None,
        turns: vec![Turn::new(Role::User, "hi")],
        modified,
    };
    let item = conversation_item(untimed, MemoryMeta::default()).unwrap();
    assert_eq!(item.meta().observed_at, modified);
}

#[tokio::test]
async fn folder_items_carry_workspace_folder_path_language_mtime_and_mime() {
    let workspace = TempDir::new().unwrap();
    fs::create_dir_all(workspace.path().join("repo/src")).unwrap();
    fs::write(workspace.path().join("repo/src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(workspace.path().join("repo/README.md"), "# Repo\n\nAbout.").unwrap();
    let mut source = entry(SourceKind::Folder);
    source.path = Some("repo".into());

    let converter = ConverterChain::default();
    let collected = collect_items(&FolderReader, &source, workspace.path(), &converter)
        .await
        .unwrap();
    assert!(collected.skipped.is_empty(), "{:?}", collected.skipped);
    assert_eq!(collected.items.len(), 2);

    let canonical_root = fs::canonicalize(workspace.path().join("repo")).unwrap();
    let rust = collected
        .items
        .iter()
        .find(|item| item.meta().language.as_deref() == Some("rust"))
        .unwrap();
    let (title, body, mime, meta) = document_parts(rust);
    assert_eq!(title.as_deref(), Some("main.rs"));
    assert_eq!(body, "fn main() {}\n");
    assert_eq!(mime.as_deref(), Some("text/x-source"));
    assert_eq!(meta.source.kind, Api::Folder);
    assert_eq!(meta.source.id.as_deref(), Some("src_test"));
    assert_eq!(
        meta.workspace.as_deref(),
        Some(workspace.path().display().to_string().as_str())
    );
    assert_eq!(
        meta.file_path.as_deref(),
        Some(
            canonical_root
                .join("src/main.rs")
                .display()
                .to_string()
                .as_str()
        )
    );
    assert_eq!(
        meta.folder.as_deref(),
        Some(canonical_root.join("src").display().to_string().as_str())
    );
    assert!(meta.observed_at.is_some(), "mtime becomes observed_at");

    let readme = collected
        .items
        .iter()
        .find(|item| item.meta().language.is_none())
        .unwrap();
    let (title, _, mime, _) = document_parts(readme);
    assert_eq!(title.as_deref(), Some("Repo"));
    assert_eq!(mime.as_deref(), Some("text/markdown"));
}

#[tokio::test]
async fn a_file_no_converter_handles_is_skipped_not_fatal() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("ok.md"), "fine").unwrap();
    fs::write(workspace.path().join("scan.pdf"), b"%PDF-1.7\nbinary").unwrap();
    let mut source = entry(SourceKind::Folder);
    source.path = Some(workspace.path().display().to_string());
    source.glob = Some("*".into());

    let collected = collect_items(
        &FolderReader,
        &source,
        workspace.path(),
        &ConverterChain::default(),
    )
    .await
    .unwrap();
    assert_eq!(collected.items.len(), 1);
    assert_eq!(collected.skipped.len(), 1);
    assert_eq!(collected.skipped[0].id, "scan.pdf");
    assert!(matches!(
        collected.skipped[0].error,
        Error::Document(crate::documents::Error::UnsupportedFormat(_))
    ));
}

#[tokio::test]
async fn a_configured_file_source_yields_one_file_item() {
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("plan.md"), "# Plan\n\nShip.").unwrap();
    let mut source = entry(SourceKind::File);
    source.path = Some("plan.md".into());

    let collected = collect_items(
        &FileReader,
        &source,
        workspace.path(),
        &ConverterChain::default(),
    )
    .await
    .unwrap();
    assert_eq!(collected.items.len(), 1);
    let meta = collected.items[0].meta();
    assert_eq!(meta.source.kind, Api::File);
    assert_eq!(meta.source.id.as_deref(), Some("src_test"));
    assert!(meta.file_path.as_deref().unwrap().ends_with("plan.md"));
}

#[tokio::test]
async fn file_item_reads_a_path_with_no_configured_source() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("query.sql");
    fs::write(&path, "SELECT 1;").unwrap();

    let item = file_item(&path, Some("/work"), None, &ConverterChain::default())
        .await
        .unwrap();
    let (title, body, _, meta) = document_parts(&item);
    assert_eq!(title.as_deref(), Some("query.sql"));
    assert_eq!(body, "SELECT 1;");
    assert_eq!(meta.source.kind, Api::File);
    assert_eq!(meta.source.id, None);
    assert_eq!(meta.workspace.as_deref(), Some("/work"));
    assert_eq!(meta.language.as_deref(), Some("sql"));

    let missing = file_item(
        &dir.path().join("nope.md"),
        None,
        None,
        &ConverterChain::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, Error::NotFound(_)), "got {missing:?}");
}

#[tokio::test]
async fn conversation_sources_yield_conversation_items_with_roles_and_times() {
    let workspace = TempDir::new().unwrap();
    let threads = workspace.path().join("threads");
    fs::create_dir_all(&threads).unwrap();
    fs::write(
        threads.join("t_1.json"),
        serde_json::json!({
            "title": "Chat",
            "messages": [
                { "role": "user", "content": "hi", "created_at": "2024-05-21T12:00:00Z" },
                { "role": "assistant", "content": "hello", "created_at": 1716292860000_i64 },
                { "role": "user", "content": "" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    fs::write(threads.join("empty.json"), r#"{"messages":[]}"#).unwrap();

    let collected = collect_items(
        &ConversationReader,
        &entry(SourceKind::Conversation),
        workspace.path(),
        &ConverterChain::default(),
    )
    .await
    .unwrap();
    assert_eq!(collected.items.len(), 1);
    assert_eq!(collected.skipped.len(), 1, "the empty thread is skipped");

    let StoreItem::Conversation { turns, meta } = &collected.items[0] else {
        panic!("expected a conversation");
    };
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].role, Role::User);
    assert_eq!(turns[1].role, Role::Assistant);
    assert_eq!(turns[0].at.map(|at| at.timestamp()), Some(1_716_292_800));
    assert_eq!(meta.source.kind, Api::Conversation);
    assert_eq!(meta.thread_id.as_deref(), Some("t_1"));
    assert_eq!(meta.turns, Some(TurnRange { first: 0, last: 1 }));
    assert_eq!(meta.observed_at, turns[1].at);
}

/// A reader whose listing fails, to prove `collect_items` reports it.
#[derive(Debug)]
struct BrokenReader;

#[async_trait]
impl SourceReader for BrokenReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }

    async fn list_items(&self, _: &MemorySourceEntry, _: &Path) -> Result<Vec<SourceItem>> {
        Err(Error::Unreachable("down".into()))
    }

    async fn read_item(&self, _: &MemorySourceEntry, _: &str, _: &Path) -> Result<SourceContent> {
        Err(Error::Unreachable("down".into()))
    }
}

#[tokio::test]
async fn a_failed_listing_fails_the_collect_pass() {
    let error = collect_items(
        &BrokenReader,
        &entry(SourceKind::Folder),
        Path::new("/unused"),
        &ConverterChain::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::Unreachable(_)));
}

/// A reader that serves fixed content, to exercise the default
/// `read_store_item`.
#[derive(Debug)]
struct FixedReader;

#[async_trait]
impl SourceReader for FixedReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }

    async fn list_items(&self, _: &MemorySourceEntry, _: &Path) -> Result<Vec<SourceItem>> {
        Ok(vec![SourceItem {
            id: "https://example.com".into(),
            title: "Example".into(),
            updated_at_ms: Some(1_000),
        }])
    }

    async fn read_item(&self, _: &MemorySourceEntry, id: &str, _: &Path) -> Result<SourceContent> {
        Ok(content(
            id,
            "# Example\n\ntext",
            ContentType::Markdown,
            serde_json::json!({ "url": id }),
        ))
    }
}

#[tokio::test]
async fn the_default_read_store_item_maps_reader_content() {
    let collected = collect_items(
        &FixedReader,
        &entry(SourceKind::Folder),
        Path::new("/unused"),
        &ConverterChain::default(),
    )
    .await
    .unwrap();
    let meta = collected.items[0].meta();
    assert_eq!(meta.source.kind, Api::Folder);
    assert_eq!(
        meta.observed_at.map(|at| at.timestamp_millis()),
        Some(1_000)
    );
}
