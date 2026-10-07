//! Imports v1 workspaces built from the verbatim v1 DDL and checks every
//! mapping, the order, and resumption.

// Fixture helpers in `support` build databases and fail loudly on setup errors.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::Path;

use support::{OLD_MEMORY_DDL, chunk, chunk_store, doc, facet, turn, workspace};
use tinymemory_api::{
    DocumentBody, LearningKind, Role, SourceKind, StoreItem, ToolCallRef, TurnRange,
};
use tinymemory_integrations::import::{
    Checkpoint, ChunkCursor, EXTERNAL_SYNC_TAG, Error, ImportedItem, LegacyCounts, LegacyWorkspace,
};

const T0: f64 = 1_700_000_000.0;

/// A workspace exercising every section and edge case.
fn rich() -> tempfile::TempDir {
    let (dir, conn) = workspace(support::MEMORY_DDL);
    doc(
        &conn,
        "d01",
        "document_notes",
        Some("document:notes"),
        "Plan",
        "Ship v2.",
        r#"["work"]"#,
        r#"{"url": "https://x.test/plan", "mime": "text/markdown"}"#,
        T0 + 0.5,
    );
    doc(
        &conn,
        "d02",
        "source_gh",
        Some("source:gh"),
        "README",
        "readme",
        "[]",
        "{}",
        T0,
    );
    doc(
        &conn,
        "d03",
        "event_chat",
        Some("event:chat"),
        "evt",
        r#"{"kind": "message"}"#,
        "[]",
        "{}",
        T0,
    );
    doc(
        &conn,
        "d04",
        "learning_style",
        None,
        "verbosity",
        r#"{"class":"style","key":"verbosity","value":"terse","cue_family":"explicit",
            "evidence":{"type":"episodic","episodic_id":42},"initial_confidence":0.8,
            "observed_at":1600000000.0}"#,
        "[]",
        "{}",
        T0,
    );
    doc(
        &conn,
        "d05",
        "global",
        Some("global"),
        "home",
        "User lives in Lisbon.",
        "[]",
        "{}",
        T0,
    );
    doc(
        &conn,
        "d06",
        "learning_identity",
        Some("learning:identity"),
        "x",
        "not json at all",
        "[]",
        "{}",
        T0 + 6.0,
    );
    doc(
        &conn,
        "d07",
        "user_notes",
        None,
        "user_notes",
        "remember milk",
        "not json",
        "",
        T0,
    );
    doc(
        &conn,
        "d08",
        "learning_tooling",
        Some("learning:tooling"),
        "shell",
        r#"{"class":"tooling","key":"shell","value":{"prefers":"zsh"},"initial_confidence":1.5}"#,
        "[]",
        "{}",
        T0 + 8.0,
    );
    doc(
        &conn,
        "d09",
        "document_blank",
        None,
        "blank",
        "   ",
        "[]",
        "{}",
        T0,
    );
    doc(
        &conn,
        "d10",
        "learning_veto",
        Some("learning:veto"),
        "emoji",
        r#"{"class":"veto","key":"emoji","value":"never","initial_confidence":0.6}"#,
        "[]",
        "{}",
        T0,
    );

    // Two threads interleaved in time.
    turn(&conn, "t-b", 100.0, "user", "b: hello", None);
    turn(&conn, "t-a", 101.0, "user", "a: hi", None);
    turn(
        &conn,
        "t-b",
        102.0,
        "assistant",
        "b: searching",
        Some(r#"[{"name":"search","id":"c1"}]"#),
    );
    turn(
        &conn,
        "t-a",
        103.0,
        "assistant",
        "a: hello back",
        Some("{oops"),
    );
    turn(&conn, "t-a", 103.0, "Tool", "a: tool output", None);
    turn(&conn, "t-a", 104.0, "narrator", "a: aside", None);
    turn(&conn, "t-a", 105.0, "user", "  ", None);

    facet(
        &conn,
        "f1",
        "preference",
        "tone",
        "terse",
        0.9,
        T0,
        "active",
        "pinned",
        Some("style"),
    );
    facet(
        &conn,
        "f2",
        "skill",
        "rust",
        "expert",
        1.2,
        T0,
        "provisional",
        "auto",
        None,
    );
    facet(
        &conn,
        "f3",
        "preference",
        "font",
        "serif",
        0.4,
        T0,
        "dropped",
        "auto",
        None,
    );
    facet(
        &conn,
        "f4",
        "role",
        "job",
        "pilot",
        0.7,
        T0,
        "active",
        "forgotten",
        None,
    );

    let chunks = chunk_store(dir.path());
    std::fs::write(
        dir.path().join("memory_tree/content/e1-1.md"),
        "full second part",
    )
    .unwrap();
    chunk(
        &chunks,
        "k3",
        "email",
        "e1",
        1,
        2_000,
        "second…",
        "[\"inbox\"]",
        Some("e1-1.md"),
    );
    chunk(
        &chunks,
        "k2",
        "email",
        "e1",
        0,
        1_000,
        "first part",
        "[\"inbox\"]",
        Some("gone.md"),
    );
    chunk(&chunks, "k1", "chat", "c1", 0, 3_000, "c: one", "[]", None);
    chunk(
        &chunks, "k4", "chat", "c1", 1, 4_000, "c: two", "[\"dm\"]", None,
    );
    dir
}

fn all(ws: &LegacyWorkspace) -> Vec<ImportedItem> {
    ws.items().collect::<Result<_, _>>().expect("import")
}

fn source_id(item: &StoreItem) -> String {
    item.meta().source.id.clone().expect("legacy id")
}

fn find<'a>(items: &'a [ImportedItem], id: &str) -> &'a StoreItem {
    &items
        .iter()
        .find(|imported| source_id(&imported.item) == id)
        .expect("legacy id was imported")
        .item
}

#[test]
fn refuses_a_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = LegacyWorkspace::open(dir.path().join("nope")).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "{err:?}");
}

#[test]
fn refuses_a_directory_without_either_store() {
    let dir = tempfile::tempdir().unwrap();
    let err = LegacyWorkspace::open(dir.path()).unwrap_err();
    assert!(matches!(err, Error::NotLegacy { .. }), "{err:?}");
    assert!(
        err.to_string()
            .contains("neither memory/memory.db nor a usable memory_tree/chunks.db"),
        "{err}"
    );
}

#[test]
fn refuses_an_unusable_chunk_store_without_a_memory_db() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("memory_tree")).unwrap();
    rusqlite::Connection::open(dir.path().join("memory_tree/chunks.db"))
        .unwrap()
        .execute_batch("CREATE TABLE other (x TEXT);")
        .unwrap();
    let err = LegacyWorkspace::open(dir.path()).unwrap_err();
    assert!(matches!(err, Error::NotLegacy { .. }), "{err:?}");
}

#[test]
fn opens_a_store_with_only_a_chunk_store() {
    // The later v1 engine wrote memory_tree/chunks.db and no memory.db.
    let dir = tempfile::tempdir().unwrap();
    let chunks = chunk_store(dir.path());
    chunk(&chunks, "k1", "chat", "c1", 0, 3_000, "c: one", "[]", None);
    chunk(&chunks, "k2", "email", "e1", 0, 1_000, "hello", "[]", None);
    drop(chunks);

    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    assert!(!ws.has_memory_db());
    assert!(ws.has_chunks());
    let ids: Vec<String> = all(&ws).iter().map(|i| source_id(&i.item)).collect();
    assert_eq!(ids, ["mem_tree_chunks:chat:c1", "mem_tree_chunks:email:e1"]);
    let counts = ws.counts().unwrap();
    assert_eq!(counts.chunks, 2);
    assert_eq!(counts.total(), 2);
}

/// What `items()` yields, counted per section by legacy id and kind.
fn counted(items: &[ImportedItem]) -> LegacyCounts {
    let mut counts = LegacyCounts::default();
    for imported in items {
        let id = source_id(&imported.item);
        let slot = if id.starts_with("mem_tree_chunks:") {
            &mut counts.chunks
        } else if id.starts_with("episodic_log:") {
            &mut counts.conversations
        } else if id.starts_with("user_profile:") {
            &mut counts.profile
        } else if matches!(imported.item, StoreItem::Learning { .. }) {
            &mut counts.learnings
        } else {
            &mut counts.documents
        };
        *slot += 1;
    }
    counts
}

#[test]
fn counts_match_what_the_import_yields() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let counts = ws.counts().unwrap();
    assert_eq!(counts, counted(&all(&ws)));
    assert!(counts.documents > 0 && counts.learnings > 0 && counts.profile > 0);
    assert!(!counts.is_empty());
}

#[test]
fn counts_match_an_early_store_without_optional_columns() {
    let (dir, conn) = workspace(OLD_MEMORY_DDL);
    conn.execute_batch(
        "INSERT INTO memory_docs VALUES ('d1','document_notes','d1','t','body','chat','normal',
           '[]','{}','core',NULL,1.0,1.0,'');
         INSERT INTO memory_docs VALUES ('d2','learning_style','d2','t','x','chat','normal',
           '[]','{}','core',NULL,1.0,1.0,'');
         INSERT INTO memory_docs VALUES ('d3','event_chat','d3','t','e','chat','normal',
           '[]','{}','core',NULL,1.0,1.0,'');
         INSERT INTO user_profile (facet_id, facet_type, key, value, confidence, first_seen_at,
           last_seen_at) VALUES ('f1','preference','k','v',0.5,1.0,1.0), ('f2','preference','k2',' ',0.5,1.0,1.0);",
    )
    .unwrap();
    drop(conn);
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    assert_eq!(ws.counts().unwrap(), counted(&all(&ws)));
    assert_eq!(ws.counts().unwrap().total(), 3);
}

#[test]
fn an_empty_store_counts_nothing() {
    let (dir, conn) = workspace(support::MEMORY_DDL);
    drop(conn);
    let counts = LegacyWorkspace::open(dir.path()).unwrap().counts().unwrap();
    assert!(counts.is_empty());
}

#[test]
fn refuses_a_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    let err = LegacyWorkspace::open(&file).unwrap_err();
    assert!(matches!(err, Error::NotLegacy { .. }), "{err:?}");
}

#[test]
fn refuses_a_memory_db_that_is_not_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("memory")).unwrap();
    std::fs::write(
        dir.path().join("memory/memory.db"),
        "this is definitely not a sqlite database file, just some text",
    )
    .unwrap();
    let err = LegacyWorkspace::open(dir.path()).unwrap_err();
    assert!(matches!(err, Error::NotLegacy { .. }), "{err:?}");
}

#[test]
fn refuses_a_sqlite_store_of_another_shape() {
    let (dir, _conn) = workspace("CREATE TABLE notes (id TEXT);");
    let err = LegacyWorkspace::open(dir.path()).unwrap_err();
    match err {
        Error::NotLegacy { reason, .. } => assert!(reason.contains("memory_docs"), "{reason}"),
        other => panic!("expected NotLegacy, got {other:?}"),
    }
}

#[test]
fn an_empty_legacy_store_yields_nothing() {
    let (dir, _conn) = workspace(support::MEMORY_DDL);
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    assert!(!ws.has_chunks());
    assert_eq!(ws.items().count(), 0);
}

#[test]
fn yields_sections_and_rows_in_a_fixed_order() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    assert!(ws.has_chunks());
    let ids: Vec<String> = all(&ws).iter().map(|i| source_id(&i.item)).collect();
    assert_eq!(
        ids,
        [
            "memory_docs:d01",
            "memory_docs:d02",
            "memory_docs:d07",
            "mem_tree_chunks:chat:c1",
            "mem_tree_chunks:email:e1",
            "episodic_log:t-a",
            "episodic_log:t-b",
            "memory_docs:d04",
            "memory_docs:d05",
            "memory_docs:d06",
            "memory_docs:d08",
            "memory_docs:d10",
            "user_profile:f1",
            "user_profile:f2",
        ]
    );
}

#[test]
fn the_page_size_does_not_change_what_is_yielded() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let paged: Vec<ImportedItem> = ws
        .items()
        .with_page_size(1)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(paged, all(&ws));
    assert_eq!(all(&ws), all(&ws));
}

#[test]
fn every_item_is_a_valid_import_from_this_workspace() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let workspace_path = ws.path().display().to_string();
    assert!(Path::new(&workspace_path).is_absolute());
    for imported in all(&ws) {
        let meta = imported.item.meta();
        assert_eq!(meta.source.kind, SourceKind::Import);
        assert_eq!(meta.workspace.as_deref(), Some(workspace_path.as_str()));
        imported.item.validate().expect("storable");
    }
}

#[test]
fn maps_document_rows() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let StoreItem::Document {
        title,
        body,
        mime,
        meta,
    } = find(&items, "memory_docs:d01")
    else {
        panic!("d01 is a document");
    };
    assert_eq!(title.as_deref(), Some("Plan"));
    assert_eq!(body, &DocumentBody::Text("Ship v2.".into()));
    assert_eq!(mime.as_deref(), Some("text/markdown"));
    assert_eq!(meta.url.as_deref(), Some("https://x.test/plan"));
    assert_eq!(meta.tags, ["work", "ns:document:notes"]);
    let observed = meta.observed_at.unwrap();
    assert_eq!(observed.timestamp(), 1_700_000_000);
    assert_eq!(observed.timestamp_subsec_millis(), 500);

    // A plain Memory::store row with no logical namespace and junk JSON.
    let StoreItem::Document {
        mime, meta, body, ..
    } = find(&items, "memory_docs:d07")
    else {
        panic!("d07 is a document");
    };
    assert_eq!(body, &DocumentBody::Text("remember milk".into()));
    assert_eq!(mime, &None);
    assert_eq!(meta.url, None);
    assert_eq!(meta.tags, ["ns:user_notes"]);
    assert_eq!(
        find(&items, "memory_docs:d02").meta().tags,
        ["ns:source:gh"]
    );
}

#[test]
fn skips_raw_events_and_blank_documents() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let ids: Vec<String> = all(&ws).iter().map(|i| source_id(&i.item)).collect();
    assert!(!ids.contains(&"memory_docs:d03".to_string()));
    assert!(!ids.contains(&"memory_docs:d09".to_string()));
}

#[test]
fn maps_learning_candidates() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    // Sanitised `learning_style` with a NULL logical namespace.
    let StoreItem::Learning {
        text,
        kind,
        confidence,
        evidence,
        meta,
    } = find(&items, "memory_docs:d04")
    else {
        panic!("d04 is a learning");
    };
    assert_eq!(text, "verbosity: terse");
    assert_eq!(*kind, LearningKind::Preference);
    assert_eq!(*confidence, 0.8);
    let evidence: serde_json::Value = serde_json::from_str(evidence.as_deref().unwrap()).unwrap();
    assert_eq!(
        evidence,
        serde_json::json!({"type": "episodic", "episodic_id": 42})
    );
    assert_eq!(meta.tags, ["style"]);
    assert_eq!(meta.observed_at.unwrap().timestamp(), 1_600_000_000);

    let StoreItem::Learning {
        text,
        kind,
        confidence,
        evidence,
        meta,
    } = find(&items, "memory_docs:d08")
    else {
        panic!("d08 is a learning");
    };
    assert_eq!(text, r#"shell: {"prefers":"zsh"}"#);
    assert_eq!(*kind, LearningKind::Procedure);
    assert_eq!(*confidence, 1.0, "clamped");
    assert_eq!(evidence, &None);
    assert_eq!(
        meta.observed_at.unwrap().timestamp(),
        1_700_000_008,
        "falls back to updated_at"
    );

    let StoreItem::Learning { kind, .. } = find(&items, "memory_docs:d10") else {
        panic!("d10 is a learning");
    };
    assert_eq!(*kind, LearningKind::Correction);
}

#[test]
fn keeps_unparseable_learnings_and_global_rows_as_text() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let StoreItem::Learning {
        text,
        kind,
        confidence,
        meta,
        ..
    } = find(&items, "memory_docs:d06")
    else {
        panic!("d06 is a learning");
    };
    assert_eq!(text, "not json at all");
    assert_eq!(*kind, LearningKind::Other);
    assert_eq!(*confidence, 0.5);
    assert_eq!(meta.tags, ["identity"]);

    let StoreItem::Learning {
        text,
        kind,
        confidence,
        meta,
        ..
    } = find(&items, "memory_docs:d05")
    else {
        panic!("d05 is a learning");
    };
    assert_eq!(text, "User lives in Lisbon.");
    assert_eq!(*kind, LearningKind::Fact);
    assert_eq!(*confidence, 0.5);
    assert_eq!(meta.tags, ["global"]);
}

#[test]
fn maps_episodic_threads_to_conversations() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let StoreItem::Conversation { turns, meta } = find(&items, "episodic_log:t-a") else {
        panic!("t-a is a conversation");
    };
    let rendered: Vec<(Role, &str)> = turns.iter().map(|t| (t.role, t.text.as_str())).collect();
    assert_eq!(
        rendered,
        [
            (Role::User, "a: hi"),
            (Role::Assistant, "a: hello back"),
            (Role::Tool, "a: tool output"),
            (Role::User, "a: aside"),
        ]
    );
    assert!(
        turns[1].tool_calls.is_empty(),
        "unparseable tool calls are dropped"
    );
    assert_eq!(turns[0].at.unwrap().timestamp(), 101);
    assert_eq!(meta.thread_id.as_deref(), Some("t-a"));
    assert_eq!(meta.turns, Some(TurnRange { first: 0, last: 3 }));
    assert_eq!(meta.observed_at.unwrap().timestamp(), 104);

    let StoreItem::Conversation { turns, meta } = find(&items, "episodic_log:t-b") else {
        panic!("t-b is a conversation");
    };
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[1].tool_calls,
        [ToolCallRef {
            name: "search".into(),
            id: Some("c1".into()),
        }]
    );
    assert_eq!(meta.turns, Some(TurnRange { first: 0, last: 1 }));
}

#[test]
fn maps_live_profile_facets_to_preferences() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let StoreItem::Learning {
        text,
        kind,
        confidence,
        evidence,
        meta,
    } = find(&items, "user_profile:f1")
    else {
        panic!("f1 is a learning");
    };
    assert_eq!(text, "tone: terse");
    assert_eq!(*kind, LearningKind::Preference);
    assert_eq!(*confidence, 0.9);
    assert_eq!(evidence.as_deref(), Some(r#"["seg-1"]"#));
    assert_eq!(meta.tags, ["preference", "style"]);
    assert_eq!(meta.observed_at.unwrap().timestamp(), 1_700_000_000);

    let StoreItem::Learning {
        confidence, meta, ..
    } = find(&items, "user_profile:f2")
    else {
        panic!("f2 is a learning");
    };
    assert_eq!(*confidence, 1.0);
    assert_eq!(meta.tags, ["skill"]);

    let ids: Vec<String> = items.iter().map(|i| source_id(&i.item)).collect();
    assert!(!ids.contains(&"user_profile:f3".to_string()), "dropped");
    assert!(!ids.contains(&"user_profile:f4".to_string()), "forgotten");
}

#[test]
fn maps_chunk_sources() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let StoreItem::Document { body, meta, .. } = find(&items, "mem_tree_chunks:email:e1") else {
        panic!("email is a document");
    };
    // seq 0 has a missing content file (preview kept); seq 1 reads its file.
    assert_eq!(
        body,
        &DocumentBody::Text("first part\n\nfull second part".into())
    );
    assert_eq!(meta.tags, ["inbox", "source_kind:email"]);
    assert_eq!(meta.observed_at.unwrap().timestamp_millis(), 2_000);
    assert_eq!(meta.thread_id, None);

    let StoreItem::Conversation { turns, meta } = find(&items, "mem_tree_chunks:chat:c1") else {
        panic!("chat is a conversation");
    };
    let texts: Vec<&str> = turns.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["c: one", "c: two"]);
    assert!(turns.iter().all(|t| t.role == Role::User));
    assert_eq!(meta.thread_id.as_deref(), Some("c1"));
    assert_eq!(meta.turns, Some(TurnRange { first: 0, last: 1 }));
    assert_eq!(meta.tags, ["dm", "source_kind:chat"]);
}

#[test]
fn resuming_from_any_checkpoint_yields_exactly_the_remainder() {
    let dir = rich();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let everything = all(&ws);
    let from_start: Vec<ImportedItem> = ws
        .items_from(&Checkpoint::default())
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(from_start, everything);
    for (index, imported) in everything.iter().enumerate() {
        let persisted = imported.checkpoint.to_json().unwrap();
        let restored = Checkpoint::from_json(&persisted).unwrap();
        for page_size in [1, 3, 256] {
            let rest: Vec<ImportedItem> = ws
                .items_from(&restored)
                .with_page_size(page_size)
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(rest, everything[index + 1..], "resume after item {index}");
        }
    }
    let last = &everything.last().unwrap().checkpoint;
    assert_eq!(last.documents.as_deref(), Some("d07"));
    assert_eq!(
        last.chunks,
        Some(ChunkCursor {
            source_kind: "email".into(),
            source_id: "e1".into(),
        })
    );
    assert_eq!(last.conversations.as_deref(), Some("t-b"));
    assert_eq!(last.learnings.as_deref(), Some("d10"));
    assert_eq!(last.profile.as_deref(), Some("f2"));
}

#[test]
fn tags_externally_synced_rows_in_every_memory_docs_section() {
    let (dir, conn) = workspace(support::MEMORY_DDL);
    let rows = [
        // (id, namespace, logical, content, taint)
        (
            "e1",
            "document_gmail",
            "document:gmail",
            "Invoice due Friday",
            "external_sync",
        ),
        (
            "e2",
            "learning_style",
            "learning:style",
            r#"{"class":"style","key":"tone","value":"terse"}"#,
            "external_sync",
        ),
        (
            "e3",
            "global",
            "global",
            "Always cc finance",
            "external_sync",
        ),
        (
            "e4",
            "document_web",
            "document:web",
            "Unknown taint",
            "sideloaded",
        ),
        ("e5", "document_web", "document:web", "Blank taint", ""),
        (
            "i1",
            "document_notes",
            "document:notes",
            "My own note",
            "internal",
        ),
        (
            "i2",
            "document_notes",
            "document:notes",
            "Spelled loosely",
            " Internal ",
        ),
    ];
    for (id, namespace, logical, content, taint) in rows {
        doc(
            &conn,
            id,
            namespace,
            Some(logical),
            "",
            content,
            "[]",
            "{}",
            T0,
        );
        conn.execute(
            "UPDATE memory_docs SET taint = ?2 WHERE document_id = ?1",
            rusqlite::params![id, taint],
        )
        .unwrap();
    }
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    let tags = |id: &str| {
        find(&items, &format!("memory_docs:{id}"))
            .meta()
            .tags
            .clone()
    };

    assert_eq!(tags("e1"), ["ns:document:gmail", EXTERNAL_SYNC_TAG]);
    assert!(matches!(
        find(&items, "memory_docs:e2"),
        StoreItem::Learning { .. }
    ));
    assert_eq!(tags("e2"), ["style", EXTERNAL_SYNC_TAG]);
    assert_eq!(tags("e3"), ["global", EXTERNAL_SYNC_TAG]);
    // v1 decodes anything but `internal` as external; so does the importer.
    assert!(tags("e4").contains(&EXTERNAL_SYNC_TAG.to_string()));
    assert!(tags("e5").contains(&EXTERNAL_SYNC_TAG.to_string()));
    assert_eq!(tags("i1"), ["ns:document:notes"]);
    assert_eq!(tags("i2"), ["ns:document:notes"]);
}

#[test]
fn a_store_without_the_taint_column_reads_as_internal() {
    let (dir, conn) = workspace(OLD_MEMORY_DDL);
    conn.execute_batch(
        "INSERT INTO memory_docs (document_id, namespace, key, title, content, source_type,
           priority, tags_json, metadata_json, category, created_at, updated_at, markdown_rel_path)
         VALUES ('a', 'document_old', 'k', 'Old', 'old body', 'doc', 'n', '[]', '{}', 'core', 1, 1, '');",
    )
    .unwrap();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    assert!(items.iter().all(|imported| {
        !imported
            .item
            .meta()
            .tags
            .iter()
            .any(|t| t == EXTERNAL_SYNC_TAG)
    }));
}

#[test]
fn imports_an_early_v1_store_without_optional_columns() {
    let (dir, conn) = workspace(OLD_MEMORY_DDL);
    conn.execute_batch(
        "INSERT INTO memory_docs (document_id, namespace, key, title, content, source_type,
           priority, tags_json, metadata_json, category, created_at, updated_at, markdown_rel_path)
         VALUES
           ('a', 'document_old', 'k', 'Old', 'old body', 'doc', 'n', '[]', '{}', 'core', 1, 1, ''),
           ('b', 'learning_goal', 'g', 'g',
            '{\"class\":\"goal\",\"key\":\"ship\",\"value\":\"v2\"}', 'chat', 'n', '[]', '{}',
            'core', 1, 1, '');
         INSERT INTO episodic_log (session_id, timestamp, role, content)
           VALUES ('s', 1.0, 'user', 'hi');
         INSERT INTO user_profile (facet_id, facet_type, key, value, first_seen_at, last_seen_at)
           VALUES ('p', 'context', 'city', 'Lisbon', 1, 2);",
    )
    .unwrap();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let items = all(&ws);
    assert_eq!(items.len(), 4);
    assert_eq!(
        find(&items, "memory_docs:a").meta().tags,
        ["ns:document:old"]
    );
    let StoreItem::Learning {
        text,
        kind,
        confidence,
        ..
    } = find(&items, "memory_docs:b")
    else {
        panic!("b is a learning");
    };
    assert_eq!(text, "ship: v2");
    assert_eq!(*kind, LearningKind::Other);
    assert_eq!(*confidence, 0.5, "no initial_confidence");
    assert!(matches!(
        find(&items, "episodic_log:s"),
        StoreItem::Conversation { .. }
    ));
    let StoreItem::Learning {
        text, confidence, ..
    } = find(&items, "user_profile:p")
    else {
        panic!("p is a learning");
    };
    assert_eq!(text, "city: Lisbon");
    assert_eq!(*confidence, 0.5, "column default");
}

#[test]
fn a_chunk_store_without_its_table_is_skipped() {
    let (dir, _conn) = workspace(support::MEMORY_DDL);
    std::fs::create_dir_all(dir.path().join("memory_tree")).unwrap();
    rusqlite::Connection::open(dir.path().join("memory_tree/chunks.db"))
        .unwrap()
        .execute_batch("CREATE TABLE other (x TEXT);")
        .unwrap();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    assert!(!ws.has_chunks());
    assert_eq!(ws.items().count(), 0);
}

#[test]
fn an_unreadable_chunk_body_fails_once_then_stops() {
    let (dir, _conn) = workspace(support::MEMORY_DDL);
    let chunks = chunk_store(dir.path());
    std::fs::create_dir_all(dir.path().join("memory_tree/content/dir.md")).unwrap();
    chunk(
        &chunks,
        "k",
        "document",
        "x",
        0,
        1,
        "preview",
        "[]",
        Some("dir.md"),
    );
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let mut items = ws.items();
    assert!(matches!(items.next(), Some(Err(Error::Io { .. }))));
    assert!(items.next().is_none());
}

#[test]
fn a_row_sqlite_cannot_decode_is_a_sqlite_error() {
    let (dir, conn) = workspace(support::MEMORY_DDL);
    conn.execute_batch(
        "INSERT INTO user_profile (facet_id, facet_type, key, value, first_seen_at, last_seen_at)
           VALUES (X'00FF', 'context', 'k', 'v', 1, 1);",
    )
    .unwrap();
    let ws = LegacyWorkspace::open(dir.path()).unwrap();
    let mut items = ws.items();
    assert!(matches!(items.next(), Some(Err(Error::Sqlite(_)))));
    assert!(items.next().is_none());
}

// --- migrate: a v1 workspace into an engine, in resumable batches ---

mod migration {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tinymemory_api::conformance::ReferenceEngine;
    use tinymemory_api::{
        EngineDescriptor, EngineHealth, FetchPage, FetchRequest, ForgetReport, ForgetTarget,
        ListPage, ListRequest, MAX_STORE_MANY, MemoryEngine, RecallAnswer, RecallRequest,
        StoreItem, StoreReceipt, async_trait,
    };
    use tinymemory_integrations::import::{
        Checkpoint, Error, LegacyWorkspace, MigrationReport, migrate, migrate_with,
    };

    use super::T0;
    use super::support::{doc, workspace};

    /// More documents than two full `store_many` batches hold.
    const COUNT: usize = 2 * MAX_STORE_MANY + 50;

    /// A workspace of `COUNT` documents, `d000` to `d249` in key order.
    fn documents() -> (tempfile::TempDir, LegacyWorkspace) {
        let (dir, conn) = workspace(super::support::MEMORY_DDL);
        for index in 0..COUNT {
            doc(
                &conn,
                &format!("d{index:03}"),
                "document_notes",
                None,
                &format!("Note {index}"),
                &format!("Body of note {index}."),
                "[]",
                "{}",
                T0 + index as f64,
            );
        }
        drop(conn);
        let legacy = LegacyWorkspace::open(dir.path()).unwrap();
        (dir, legacy)
    }

    fn key(checkpoint: &Checkpoint) -> Option<&str> {
        checkpoint.documents.as_deref()
    }

    /// Delegates to a [`ReferenceEngine`], failing the `fail_on`th
    /// `store_many` call (1-based) without storing anything.
    struct FailingOn {
        inner: ReferenceEngine,
        fail_on: usize,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl MemoryEngine for FailingOn {
        fn descriptor(&self) -> &EngineDescriptor {
            self.inner.descriptor()
        }
        async fn health(&self) -> EngineHealth {
            self.inner.health().await
        }
        async fn recall(&self, req: RecallRequest) -> tinymemory_api::Result<RecallAnswer> {
            self.inner.recall(req).await
        }
        async fn fetch(&self, req: FetchRequest) -> tinymemory_api::Result<FetchPage> {
            self.inner.fetch(req).await
        }
        async fn store(&self, item: StoreItem) -> tinymemory_api::Result<StoreReceipt> {
            self.inner.store(item).await
        }
        async fn store_many(
            &self,
            items: Vec<StoreItem>,
        ) -> tinymemory_api::Result<Vec<StoreReceipt>> {
            if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_on {
                return Err(tinymemory_api::Error::Unavailable("engine down".into()));
            }
            self.inner.store_many(items).await
        }
        async fn forget(&self, target: ForgetTarget) -> tinymemory_api::Result<ForgetReport> {
            self.inner.forget(target).await
        }
        async fn list(&self, req: ListRequest) -> tinymemory_api::Result<ListPage> {
            self.inner.list(req).await
        }
    }

    #[tokio::test]
    async fn a_full_migration_stores_every_item_in_bounded_batches() {
        let (_dir, legacy) = documents();
        let engine = ReferenceEngine::new();
        let report = migrate(&engine, legacy, None).await.unwrap();
        assert_eq!(
            report,
            MigrationReport {
                stored: COUNT,
                replayed: 0,
                batches: 3,
                checkpoint: Checkpoint {
                    documents: Some("d249".into()),
                    ..Checkpoint::default()
                },
            }
        );
        assert_eq!(engine.len(), COUNT);
    }

    #[tokio::test]
    async fn a_second_run_is_all_replays() {
        let (dir, legacy) = documents();
        let engine = ReferenceEngine::new();
        migrate(&engine, legacy, None).await.unwrap();
        let again = migrate(&engine, LegacyWorkspace::open(dir.path()).unwrap(), None)
            .await
            .unwrap();
        assert_eq!((again.stored, again.replayed), (0, COUNT));
        assert_eq!(engine.len(), COUNT, "a re-run replays, it never duplicates");
    }

    #[tokio::test]
    async fn resuming_from_a_checkpoint_stores_only_the_rest() {
        let (_dir, legacy) = documents();
        let mid = legacy.items().nth(149).unwrap().unwrap().checkpoint;
        assert_eq!(key(&mid), Some("d149"));
        let engine = ReferenceEngine::new();
        let report = migrate(&engine, legacy, Some(mid)).await.unwrap();
        assert_eq!((report.stored, report.batches), (COUNT - 150, 1));
        assert_eq!(engine.len(), COUNT - 150);
        assert_eq!(key(&report.checkpoint), Some("d249"));
    }

    #[tokio::test]
    async fn the_callback_sees_each_committed_checkpoint_in_order() {
        let (_dir, legacy) = documents();
        let engine = ReferenceEngine::new();
        let mut seen = Vec::new();
        let report = migrate_with(&engine, legacy, None, |checkpoint: &Checkpoint| {
            seen.push(checkpoint.clone());
        })
        .await
        .unwrap();
        let keys: Vec<Option<&str>> = seen.iter().map(key).collect();
        assert_eq!(keys, [Some("d099"), Some("d199"), Some("d249")]);
        assert_eq!(seen.last(), Some(&report.checkpoint));
    }

    #[tokio::test]
    async fn an_engine_failure_carries_the_last_committed_checkpoint() {
        let (dir, legacy) = documents();
        let engine = FailingOn {
            inner: ReferenceEngine::new(),
            fail_on: 2,
            calls: AtomicUsize::new(0),
        };
        let error = migrate(&engine, legacy, None).await.unwrap_err();
        let checkpoint = error.checkpoint().cloned().unwrap();
        assert_eq!(key(&checkpoint), Some("d099"));
        match &error {
            Error::Engine { source, .. } => {
                assert!(matches!(source, tinymemory_api::Error::Unavailable(_)));
            }
            other => panic!("expected an engine error, got {other}"),
        }
        assert!(error.to_string().contains("engine down"), "{error}");
        assert_eq!(engine.inner.len(), MAX_STORE_MANY);

        let resumed = migrate(
            &engine,
            LegacyWorkspace::open(dir.path()).unwrap(),
            Some(checkpoint),
        )
        .await
        .unwrap();
        assert_eq!(
            (resumed.stored, resumed.replayed),
            (COUNT - MAX_STORE_MANY, 0)
        );
        assert_eq!(engine.inner.len(), COUNT);
    }

    #[tokio::test]
    async fn an_engine_failure_on_the_first_batch_carries_the_starting_checkpoint() {
        let (_dir, legacy) = documents();
        let engine = FailingOn {
            inner: ReferenceEngine::new(),
            fail_on: 1,
            calls: AtomicUsize::new(0),
        };
        let start = Checkpoint {
            documents: Some("d009".into()),
            ..Checkpoint::default()
        };
        let error = migrate(&engine, legacy, Some(start.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.checkpoint(), Some(&start));
    }

    #[tokio::test]
    async fn an_empty_workspace_migrates_nothing() {
        let (dir, conn) = workspace(super::support::MEMORY_DDL);
        drop(conn);
        let engine = ReferenceEngine::new();
        let report = migrate(&engine, LegacyWorkspace::open(dir.path()).unwrap(), None)
            .await
            .unwrap();
        assert_eq!(report, MigrationReport::default());
        assert_eq!(engine.len(), 0);
    }

    #[test]
    fn a_migration_can_run_on_a_spawned_task() {
        fn assert_send<T: Send>(_: &T) {}
        let (_dir, legacy) = documents();
        let engine = ReferenceEngine::new();
        assert_send(&migrate(&engine, legacy, None));
    }
}
