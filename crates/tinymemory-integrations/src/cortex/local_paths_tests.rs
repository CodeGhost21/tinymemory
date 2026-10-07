//! No local path leaves the machine (`envelope`, "Local paths"): over both
//! wires, nothing written names a file's folders, an item's folder or an
//! absolute workspace, and filters on those paths still match.

use tinymemory_api::{
    ListRequest, MemoryEngine, MemoryMeta, MetaFilter, Role, SourceKind, StoreItem, Turn,
};

use crate::cortex::testing::{Shared, both};

/// Folder names that must never reach the engine.
const PRIVATE: [&str; 3] = ["/Users/priya", "Priya-Documents", "acme-deal"];

/// Metadata with every local path set, as a desktop host fills it.
fn local_meta() -> MemoryMeta {
    let mut meta = MemoryMeta::from_source(SourceKind::File, Some("notes".into()));
    meta.file_path = Some("/Users/priya/Priya-Documents/acme-deal/pricing-v3.md".into());
    meta.folder = Some("/Users/priya/Priya-Documents/acme-deal".into());
    meta.workspace = Some("/Users/priya/Priya-Documents".into());
    meta
}

/// One item of each kind carrying [`local_meta`].
fn local_items() -> Vec<StoreItem> {
    vec![
        StoreItem::document("Pricing moved to the v3 sheet.", local_meta()),
        StoreItem::learning(
            "Prefers short answers.",
            tinymemory_api::LearningKind::Preference,
            0.9,
            local_meta(),
        ),
        StoreItem::Conversation {
            turns: vec![Turn::new(Role::User, "Where is the pricing sheet?")],
            meta: local_meta(),
        },
    ]
}

/// Everything the double was sent: every request line and every write body.
fn everything_sent(state: &Shared) -> String {
    let seen = state.seen.lock().unwrap();
    let mut sent = seen.requests.join("\n");
    for body in &seen.writes {
        sent.push('\n');
        sent.push_str(&body.to_string());
    }
    sent
}

fn assert_no_local_path(sent: &str, wire: &str) {
    assert!(!sent.is_empty(), "{wire}: nothing was recorded");
    for private in PRIVATE {
        assert!(!sent.contains(private), "{wire}: `{private}` was sent");
    }
}

#[tokio::test]
async fn no_write_on_either_wire_names_a_local_folder() {
    for (engine, state) in both().await {
        let wire = format!("{:?}", engine.wire());
        engine.store_many(local_items()).await.unwrap();
        let sent = everything_sent(&state);
        assert_no_local_path(&sent, &wire);
        assert!(
            sent.contains("file:pricing-v3.md"),
            "{wire}: the file's name is kept"
        );
    }
}

#[tokio::test]
async fn reads_give_back_the_file_name_and_no_local_folder() {
    for (engine, _state) in both().await {
        let wire = format!("{:?}", engine.wire());
        engine.store_many(local_items()).await.unwrap();
        let page = engine
            .list(ListRequest::new(MetaFilter::default(), 10))
            .await
            .unwrap();
        assert_eq!(page.items.len(), 3, "{wire}");
        for hit in &page.items {
            assert_eq!(
                hit.meta.file_path.as_deref(),
                Some("pricing-v3.md"),
                "{wire}"
            );
            assert_eq!(hit.meta.folder, None, "{wire}");
            assert_eq!(hit.meta.workspace, None, "{wire}");
        }
    }
}

#[tokio::test]
async fn filters_on_the_full_paths_still_match() {
    for (engine, _state) in both().await {
        let wire = format!("{:?}", engine.wire());
        engine.store_many(local_items()).await.unwrap();
        let count = |filter: MetaFilter| {
            let engine = &engine;
            async move {
                engine
                    .list(ListRequest::new(filter, 10))
                    .await
                    .unwrap()
                    .items
                    .len()
            }
        };
        let cases: [(&str, MetaFilter, usize); 10] = [
            (
                "the exact file",
                MetaFilter {
                    file_path: Some("/Users/priya/Priya-Documents/acme-deal/pricing-v3.md".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                "a folder above the file",
                MetaFilter {
                    file_path: Some("/Users/priya/Priya-Documents/".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                "the file's name, which reads give back",
                MetaFilter {
                    file_path: Some("pricing-v3.md".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                "a folder that only shares a prefix",
                MetaFilter {
                    file_path: Some("/Users/priya/Priya-Doc".into()),
                    ..MetaFilter::default()
                },
                0,
            ),
            (
                "the folder",
                MetaFilter {
                    folder: Some("/Users/priya/Priya-Documents/acme-deal".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                "a folder above the folder",
                MetaFilter {
                    folder: Some("/Users".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                "another folder",
                MetaFilter {
                    folder: Some("/Users/sam".into()),
                    ..MetaFilter::default()
                },
                0,
            ),
            (
                "the workspace",
                MetaFilter {
                    workspace: Some("/Users/priya/Priya-Documents".into()),
                    ..MetaFilter::default()
                },
                3,
            ),
            (
                // The source id narrows server-side, so the workspace is
                // checked against the envelope's digest.
                "the source, in another workspace",
                MetaFilter {
                    source_id: Some("notes".into()),
                    workspace: Some("/Users/sam".into()),
                    ..MetaFilter::default()
                },
                0,
            ),
            (
                "another workspace",
                MetaFilter {
                    workspace: Some("/Users/priya".into()),
                    ..MetaFilter::default()
                },
                0,
            ),
        ];
        for (case, filter, expected) in cases {
            assert_eq!(count(filter).await, expected, "{wire}: {case}");
        }
    }
}

#[tokio::test]
async fn a_logical_workspace_and_a_relative_file_keep_their_meaning() {
    for (engine, _state) in both().await {
        let wire = format!("{:?}", engine.wire());
        let mut meta = MemoryMeta::from_source(SourceKind::File, None);
        meta.workspace = Some("team-handbook".into());
        meta.file_path = Some("docs/guide.md".into());
        engine
            .store_many(vec![StoreItem::document("Guide.", meta)])
            .await
            .unwrap();
        let page = engine
            .list(ListRequest::new(
                MetaFilter {
                    file_path: Some("docs".into()),
                    workspace: Some("team-handbook".into()),
                    ..MetaFilter::default()
                },
                10,
            ))
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "{wire}");
        let held = &page.items[0].meta;
        assert_eq!(held.workspace.as_deref(), Some("team-handbook"), "{wire}");
        assert_eq!(held.file_path.as_deref(), Some("guide.md"), "{wire}");
    }
}

#[tokio::test]
async fn windows_and_home_relative_paths_do_not_leave_either() {
    for (engine, state) in both().await {
        let wire = format!("{:?}", engine.wire());
        let mut windows = MemoryMeta::from_source(SourceKind::File, None);
        windows.file_path = Some(r"C:\Users\priya\acme-deal\pricing-v3.md".into());
        windows.workspace = Some(r"C:\Users\priya".into());
        let mut home = MemoryMeta::from_source(SourceKind::File, None);
        home.file_path = Some("~/acme-deal/pricing-v4.md".into());
        home.workspace = Some("~/acme-deal".into());
        engine
            .store_many(vec![
                StoreItem::document("Windows copy.", windows),
                StoreItem::document("Home copy.", home),
            ])
            .await
            .unwrap();
        let sent = everything_sent(&state);
        for private in ["priya", "acme-deal"] {
            assert!(!sent.contains(private), "{wire}: `{private}` was sent");
        }
        assert!(sent.contains("file:pricing-v3.md"), "{wire}");
        assert!(sent.contains("file:pricing-v4.md"), "{wire}");
    }
}

/// The real desktop path: a file read from disk and converted, then stored.
#[cfg(feature = "sources")]
#[tokio::test]
async fn a_file_read_from_disk_sends_only_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Priya-Documents").join("acme-deal");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("pricing-v3.md");
    std::fs::write(&path, "# Pricing\n\nThe v3 sheet replaces v2.\n").unwrap();
    let workspace = dir.path().display().to_string();
    for (engine, state) in both().await {
        let wire = format!("{:?}", engine.wire());
        let item = crate::sources::file_item(
            &path,
            Some(&workspace),
            None,
            &crate::documents::NativeConverter,
        )
        .await
        .unwrap();
        assert!(
            item.meta()
                .file_path
                .as_deref()
                .unwrap()
                .contains("acme-deal"),
            "the fixture carries the full path"
        );
        engine.store_many(vec![item]).await.unwrap();
        let sent = everything_sent(&state);
        assert_no_local_path(&sent, &wire);
        assert!(
            !sent.contains(&workspace),
            "{wire}: the temp folder was sent"
        );
        assert!(sent.contains("file:pricing-v3.md"), "{wire}");
    }
}

/// For every path filter on a full path, the engine (matching digests)
/// keeps exactly what `MetaFilter` keeps against the full metadata, Windows
/// paths included. (A filter on the file's name alone also matches, by
/// design: it is what reads give back.)
#[tokio::test]
async fn path_filters_agree_with_the_contract_on_the_full_paths() {
    let stored = [
        ("/Users/priya/notes/a.md", "/Users/priya/notes"),
        (r"C:\Users\priya\notes\b.md", r"C:\Users\priya\notes"),
    ];
    let filters = [
        "/Users/priya/notes/a.md",
        "/Users/priya/notes",
        "/Users/priya/notes/",
        "/Users/priya/no",
        "/Users",
        "/",
        r"C:\Users\priya\notes\b.md",
        r"C:\Users\priya\notes",
        r"C:\Users",
    ];
    for (engine, _state) in both().await {
        let wire = format!("{:?}", engine.wire());
        let items: Vec<StoreItem> = stored
            .iter()
            .map(|(file, folder)| {
                let mut meta = MemoryMeta::from_source(SourceKind::File, None);
                meta.file_path = Some((*file).into());
                meta.folder = Some((*folder).into());
                StoreItem::document(format!("Notes in {file}."), meta)
            })
            .collect();
        engine.store_many(items.clone()).await.unwrap();
        for value in filters {
            for field in ["file_path", "folder"] {
                let filter = match field {
                    "file_path" => MetaFilter {
                        file_path: Some(value.into()),
                        ..MetaFilter::default()
                    },
                    _ => MetaFilter {
                        folder: Some(value.into()),
                        ..MetaFilter::default()
                    },
                };
                let wanted = items
                    .iter()
                    .filter(|item| filter.matches(item.kind(), item.meta()))
                    .count();
                let held = engine
                    .list(ListRequest::new(filter, 10))
                    .await
                    .unwrap()
                    .items
                    .len();
                assert_eq!(held, wanted, "{wire}: {field} = {value}");
            }
        }
    }
}
